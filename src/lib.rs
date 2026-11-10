use std::path::Path;
use swc_common::sync::Lrc;
use swc_common::{SourceFile, SourceMap, Span, Spanned};
use swc_ecma_ast::{
    ArrayLit, ArrayPat, ArrowExpr, AssignExpr, AssignOp, AssignTarget, AssignTargetPat, BinExpr,
    BinaryOp, BlockStmt, CallExpr, Callee, Class, CondExpr, Decl, DoWhileStmt, Expr, FnDecl,
    ForHead, ForInStmt, ForStmt, Function, Ident, IfStmt, LabeledStmt, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, NewExpr, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, SwitchStmt, TryStmt, UnaryExpr, UnaryOp, UpdateExpr, UpdateOp,
    VarDecl, VarDeclKind, VarDeclOrExpr, VarDeclarator, WhileStmt,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};
use thiserror::Error;

pub struct Compiler<'a> {
    bytecode: Vec<u8>,
    /// names currently in scope in this function, innermost last
    locals: Vec<Local>,
    /// slots used so far in this function (slots are never reused, so closures stay valid)
    slot_count: usize,
    /// block nesting depth within this function (0 = function body)
    current_scope_depth: usize,
    /// enclosing loops / switches / labeled statements, innermost last
    breakables: Vec<Breakable>,
    /// enclosing protected `try` regions, innermost last
    try_stack: Vec<TryRegion>,
    /// labels waiting to be attached to the next loop or switch
    pending_labels: Vec<String>,
    /// scratch slot for short-lived values (never live across nested evaluation)
    temp_slot: Option<u8>,
    enclosing: Option<&'a Compiler<'a>>,
}

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to load file: {0}")]
    LoadFileError(String),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("too many locals, max number of locals supported is 256")]
    TooManyLocals,
    /// `location` is filled in (as " at file:line:col") once the span is resolved against the source map
    #[error("unsupported {what}{location}")]
    Unsupported {
        what: String,
        span: Span,
        location: String,
    },
}

type CompileResult<T = ()> = Result<T, CompileError>;

fn unsupported<T>(what: impl Into<String>, span: Span) -> CompileResult<T> {
    Err(CompileError::Unsupported {
        what: what.into(),
        span,
        location: String::new(),
    })
}

fn format_location(cm: &SourceMap, span: Span) -> String {
    let loc = cm.lookup_char_pos(span.lo);
    format!(" at {}:{}:{}", loc.file.name, loc.line, loc.col.0 + 1)
}

struct Local {
    name: String,
    depth: usize,
    slot: u8,
}

#[derive(PartialEq)]
enum BreakableKind {
    Loop,
    Switch,
    /// a labeled non-loop statement: only `break label` targets it
    Labeled,
}

struct Breakable {
    kind: BreakableKind,
    labels: Vec<String>,
    /// forward jumps to patch to the end of the statement
    breaks: Vec<usize>,
    /// forward jumps to patch to the loop's continue point
    continues: Vec<usize>,
    /// `try_stack` length when the statement started
    try_depth: usize,
}

/// A protected `try` (or `catch`, when there's a `finally`) region. Leaving it early with
/// `break` / `continue` / `return` must drop its handler and run its `finally`.
struct TryRegion {
    finally: Option<BlockStmt>,
    /// scope at the `try` statement, so the inlined `finally` resolves names correctly
    locals_len: usize,
    scope_depth: usize,
}

/// Undeclared names that resolve to the host environment. Everything else must be declared.
#[derive(Clone, Copy)]
enum HostGlobal {
    Window = 0,
    GlobalThis = 1,
}

impl HostGlobal {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "window" => Some(HostGlobal::Window),
            "globalThis" => Some(HostGlobal::GlobalThis),
            _ => None,
        }
    }
}

/// `HostGlobal` operand flag: push `undefined` instead of throwing when missing (for `typeof`)
const HOST_GLOBAL_TYPEOF: u8 = 0b10;

#[derive(Debug)]
enum Operation {
    Pop,
    Return,
    LoadFloat64(f64),
    #[allow(dead_code)]
    UInt8(u8),
    #[allow(dead_code)]
    UInt16(u16),
    #[allow(dead_code)]
    UInt32(u32),
    UInt64(u64),
    /// "kumo" (WTF-8 bytes, so lone surrogates survive)
    LoadString(Vec<u8>),
    /// true
    Bool(bool),
    /// null
    Null,
    /// `undefined`
    Undefined,
    /// /exp/flags
    Regex(String, String),
    /// x()
    Call,
    /// x.a
    GetProperty,
    /// x.a = b
    SetProperty,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `===`
    EqEqEq,
    /// `!==`
    NotEqEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `<<`
    LShift,
    /// `>>`
    RShift,
    /// `>>>`
    ZeroFillRShift,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `&`
    BitAnd,
    /// `in`
    In,
    /// `instanceof`
    InstanceOf,
    /// `**`
    Exp,
    /// Get local variable at stack offset
    GetLocal(u8),
    /// Set local variable at stack offset
    SetLocal(u8),
    /// `-`
    UnaryMinus,
    /// `+`
    UnaryPlus,
    /// `!`
    LogicalNot,
    /// `~`
    BitwiseNot,
    /// `typeof`
    TypeOf,
    /// `void`
    Void,
    /// `delete`
    Delete,
    /// Create array with n elements
    Array(u32),
    /// Create object with n properties
    Object(u32),
    /// Duplicate the top value on the stack
    Dup,
    /// Create a virtual function: arity, name, code, internal. An internal function is only
    /// ever called directly from VM code, so it gets no host wrapper (see `fn_escapes`).
    Function(u8, String, Vec<u8>, bool),
    /// Get variable from enclosing scope: depth, index
    GetUpvalue(u8, u8),
    /// Set variable in enclosing scope: depth, index
    SetUpvalue(u8, u8),
    /// obj.f(): stack is [obj, f, ...args, argc]; calls f with obj as `this`
    CallMethod,
    /// `this`
    This,
    /// host `window` / `globalThis`: operand is a `HostGlobal` id, optionally | `HOST_GLOBAL_TYPEOF`
    HostGlobal(u8),
    /// Jump forward by offset
    Jump(u32),
    /// Jump forward by offset if top of stack is falsy (does not pop)
    JumpIfFalse(u32),
    /// Jump forward by offset if top of stack is truthy (does not pop)
    JumpIfTrue(u32),
    /// Jump forward by offset if top of stack is not null/undefined (does not pop)
    JumpIfNotNullish(u32),
    /// Jump backward by offset
    Loop(u32),
    /// Duplicate the top two values: [a, b] -> [a, b, a, b]
    Dup2,
    /// The currently running function (for named function expressions)
    Callee,
    /// `arguments`
    Arguments,
    /// `new f(...args)`: stack is [f, ...args, argc]
    New,
    /// `throw value`
    Throw,
    /// Install an exception handler at a forward offset; the thrown value is pushed there
    TryStart(u32),
    /// Remove the innermost exception handler
    TryEnd,
    /// Replace an object with an array of its `for-in` keys
    ForInKeys,
    /// An array literal hole (`[1, , 3]`)
    Hole,
}

impl Operation {
    fn get_opcode(&self) -> u8 {
        match self {
            Operation::LoadString(_) => 0x01,
            Operation::LoadFloat64(_) => 0x02,
            Operation::Bool(_) => 0x03,
            Operation::Pop => 0x04,
            Operation::Null => 0x05,
            Operation::Regex(_, _) => 0x06,
            Operation::Undefined => 0x07,
            Operation::Return => 0x08,
            Operation::UInt8(_) => 0x0b,
            Operation::UInt16(_) => 0x0c,
            Operation::UInt32(_) => 0x0d,
            Operation::UInt64(_) => 0x0e,
            Operation::Call => 0x0f,
            Operation::GetProperty => 0x10,
            Operation::SetProperty => 0x11,
            Operation::EqEq => 0x12,
            Operation::NotEq => 0x13,
            Operation::EqEqEq => 0x14,
            Operation::NotEqEq => 0x15,
            Operation::Lt => 0x16,
            Operation::LtEq => 0x17,
            Operation::Gt => 0x18,
            Operation::GtEq => 0x19,
            Operation::LShift => 0x1a,
            Operation::RShift => 0x1b,
            Operation::ZeroFillRShift => 0x1c,
            Operation::Add => 0x1d,
            Operation::Sub => 0x1e,
            Operation::Mul => 0x1f,
            Operation::Div => 0x20,
            Operation::Mod => 0x21,
            Operation::BitOr => 0x22,
            Operation::BitXor => 0x23,
            Operation::BitAnd => 0x24,
            Operation::In => 0x27,
            Operation::InstanceOf => 0x28,
            Operation::Exp => 0x29,
            Operation::GetLocal(_) => 0x2b,
            Operation::SetLocal(_) => 0x2c,
            Operation::UnaryMinus => 0x2d,
            Operation::UnaryPlus => 0x2e,
            Operation::LogicalNot => 0x2f,
            Operation::BitwiseNot => 0x30,
            Operation::TypeOf => 0x31,
            Operation::Void => 0x32,
            Operation::Delete => 0x33,
            Operation::Array(_) => 0x34,
            Operation::Object(_) => 0x35,
            Operation::Dup => 0x36,
            Operation::Function(_, _, _, false) => 0x37,
            Operation::Function(_, _, _, true) => 0x4b,
            Operation::GetUpvalue(_, _) => 0x38,
            Operation::SetUpvalue(_, _) => 0x39,
            Operation::Jump(_) => 0x3a,
            Operation::JumpIfFalse(_) => 0x3b,
            Operation::JumpIfTrue(_) => 0x3c,
            Operation::JumpIfNotNullish(_) => 0x3d,
            Operation::CallMethod => 0x3e,
            Operation::This => 0x3f,
            Operation::HostGlobal(_) => 0x40,
            Operation::Loop(_) => 0x41,
            Operation::Dup2 => 0x42,
            Operation::Callee => 0x43,
            Operation::Arguments => 0x44,
            Operation::New => 0x45,
            Operation::Throw => 0x46,
            Operation::TryStart(_) => 0x47,
            Operation::TryEnd => 0x48,
            Operation::ForInKeys => 0x49,
            Operation::Hole => 0x4a,
        }
    }
}

fn load_str(s: &str) -> Operation {
    Operation::LoadString(s.as_bytes().to_vec())
}

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Compiler {
            bytecode: Vec::new(),
            locals: Vec::new(),
            slot_count: 0,
            current_scope_depth: 0,
            breakables: Vec::new(),
            try_stack: Vec::new(),
            pending_labels: Vec::new(),
            temp_slot: None,
            enclosing: None,
        }
    }

    fn new_enclosing(enclosing: &'a Compiler<'a>) -> Compiler<'a> {
        Compiler {
            enclosing: Some(enclosing),
            ..Compiler::new()
        }
    }

    pub fn compile_file(&mut self, path: &Path) -> CompileResult<Vec<u8>> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm
            .load_file(path)
            .map_err(|e| CompileError::LoadFileError(e.to_string()))?;
        self.parse_and_compile(&cm, &fm)
    }

    /// Compile JS source held in memory. `name` is used in error locations.
    pub fn compile_source(&mut self, name: &str, source: &str) -> CompileResult<Vec<u8>> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            swc_common::FileName::Custom(name.to_string()).into(),
            source.to_string(),
        );
        self.parse_and_compile(&cm, &fm)
    }

    fn parse_and_compile(&mut self, cm: &SourceMap, fm: &SourceFile) -> CompileResult<Vec<u8>> {
        let lexer = Lexer::new(
            Syntax::Es(Default::default()),
            Default::default(),
            StringInput::from(fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);

        // parsed as a module, so input is always strict mode
        let parse_error = |e: swc_ecma_parser::error::Error| {
            CompileError::ParseError(format!("{}{}", e.kind().msg(), format_location(cm, e.span())))
        };
        let module = parser.parse_module().map_err(parse_error)?;
        if let Some(e) = parser.take_errors().into_iter().next() {
            return Err(parse_error(e));
        }

        self.compile(&module).map_err(|e| match e {
            CompileError::Unsupported { what, span, .. } => CompileError::Unsupported {
                what,
                span,
                location: format_location(cm, span),
            },
            e => e,
        })
    }

    pub fn compile(&mut self, module: &Module) -> CompileResult<Vec<u8>> {
        let mut stmts = Vec::with_capacity(module.body.len());
        for item in &module.body {
            match item {
                ModuleItem::Stmt(stmt) => stmts.push(stmt),
                ModuleItem::ModuleDecl(decl) => return unsupported("import/export", decl.span()),
            }
        }
        // the module body behaves like a function body
        self.compile_body(&stmts, true, true)?;
        Ok(self.bytecode.clone())
    }

    // ---------------------------------------------------------------------------------
    // statements
    // ---------------------------------------------------------------------------------

    /// Compile a list of statements forming a function body or block.
    /// `leave_last` keeps the last expression statement's value on the stack (module body).
    fn compile_body(&mut self, stmts: &[&Stmt], hoist_vars: bool, leave_last: bool) -> CompileResult {
        self.hoist_declarations(stmts, hoist_vars)?;
        let last = stmts.len().saturating_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            self.compile_stmt(stmt, leave_last && i == last)?;
        }
        Ok(())
    }

    /// Declarations are hoisted before any code runs: `var`s (function bodies only, found
    /// anywhere in the body), then `let`/`const` and function names of this block, then
    /// function values, so code can refer to anything in scope regardless of order.
    fn hoist_declarations(&mut self, stmts: &[&Stmt], hoist_vars: bool) -> CompileResult {
        if hoist_vars {
            let mut names = Vec::new();
            let mut collector = VarCollector { names: &mut names };
            for stmt in stmts {
                stmt.visit_with(&mut collector);
            }
            for name in names {
                self.declare_if_absent(name)?;
            }
        }

        for stmt in stmts {
            match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => {
                    self.declare_lexical(var_decl)?;
                }
                Stmt::Decl(Decl::Fn(fn_decl)) => {
                    self.declare_if_absent(fn_decl.ident.sym.to_string())?;
                }
                _ => {}
            }
        }

        for stmt in stmts {
            if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
                // `stmts` is everything that can see this name, so it's enough to check
                let internal = !fn_escapes(&fn_decl.ident.sym, stmts);
                self.compile_fn_decl(fn_decl, internal)?;
            }
        }
        Ok(())
    }

    fn declare_lexical(&mut self, var_decl: &VarDecl) -> CompileResult {
        for decl in &var_decl.decls {
            if let Pat::Ident(ident) = &decl.name {
                self.declare_variable(ident.id.sym.to_string())?;
            }
        }
        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &Stmt, is_last_in_module: bool) -> CompileResult {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.compile_expr(&expr_stmt.expr)?;
                // leave the last statement's value on the stack so the test runner can read it
                if !is_last_in_module {
                    self.emit_op(Operation::Pop);
                }
            }
            Stmt::Return(return_stmt) => {
                match &return_stmt.arg {
                    Some(arg) => self.compile_expr(arg)?,
                    None => self.emit_op(Operation::Undefined),
                }
                self.emit_unwind_tries(0)?;
                self.emit_op(Operation::Return);
            }
            // compiled when hoisted by `hoist_declarations`
            Stmt::Decl(Decl::Fn(_)) => {}
            Stmt::Decl(Decl::Var(var_decl)) => self.compile_var_decl(var_decl)?,
            Stmt::Block(block_stmt) => self.compile_block(block_stmt)?,
            Stmt::Empty(_) | Stmt::Debugger(_) => {}
            Stmt::If(if_stmt) => self.compile_if(if_stmt)?,
            Stmt::While(while_stmt) => self.compile_while(while_stmt)?,
            Stmt::DoWhile(do_while) => self.compile_do_while(do_while)?,
            Stmt::For(for_stmt) => self.compile_for(for_stmt)?,
            Stmt::ForIn(for_in) => self.compile_for_in(for_in)?,
            Stmt::Labeled(labeled) => self.compile_labeled(labeled)?,
            Stmt::Break(break_stmt) => {
                let label = break_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, false, break_stmt.span)?;
                self.emit_unwind_tries(self.breakables[target].try_depth)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].breaks.push(jump);
            }
            Stmt::Continue(continue_stmt) => {
                let label = continue_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, true, continue_stmt.span)?;
                self.emit_unwind_tries(self.breakables[target].try_depth)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].continues.push(jump);
            }
            Stmt::Switch(switch_stmt) => self.compile_switch(switch_stmt)?,
            Stmt::Throw(throw_stmt) => {
                self.compile_expr(&throw_stmt.arg)?;
                self.emit_op(Operation::Throw);
            }
            Stmt::Try(try_stmt) => self.compile_try(try_stmt)?,
            _ => return unsupported(stmt_kind(stmt), stmt.span()),
        }
        Ok(())
    }

    fn compile_block(&mut self, block: &BlockStmt) -> CompileResult {
        let stmts: Vec<&Stmt> = block.stmts.iter().collect();
        self.enter_scope();
        self.compile_body(&stmts, false, false)?;
        self.exit_scope();
        Ok(())
    }

    fn compile_if(&mut self, stmt: &IfStmt) -> CompileResult {
        self.compile_expr(&stmt.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.cons, false)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        if let Some(alt) = &stmt.alt {
            self.compile_stmt(alt, false)?;
        }
        self.patch_jump(end_jump);
        Ok(())
    }

    fn compile_while(&mut self, stmt: &WhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    fn compile_do_while(&mut self, stmt: &DoWhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    fn compile_for(&mut self, stmt: &ForStmt) -> CompileResult {
        // `let` in the initializer is scoped to the loop
        self.enter_scope();
        match &stmt.init {
            Some(VarDeclOrExpr::VarDecl(var_decl)) => {
                if var_decl.kind != VarDeclKind::Var {
                    self.declare_lexical(var_decl)?;
                }
                self.compile_var_decl(var_decl)?;
            }
            Some(VarDeclOrExpr::Expr(expr)) => {
                self.compile_expr(expr)?;
                self.emit_op(Operation::Pop);
            }
            None => {}
        }

        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        let exit = match &stmt.test {
            Some(test) => {
                self.compile_expr(test)?;
                let exit = self.emit_jump(Operation::JumpIfFalse(0));
                self.emit_op(Operation::Pop);
                Some(exit)
            }
            None => None,
        };
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        if let Some(update) = &stmt.update {
            self.compile_expr(update)?;
            self.emit_op(Operation::Pop);
        }
        self.emit_loop(start);
        if let Some(exit) = exit {
            self.patch_jump(exit);
            self.emit_op(Operation::Pop);
        }
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    fn compile_for_in(&mut self, stmt: &ForInStmt) -> CompileResult {
        self.enter_scope();

        // keys array and index live in hidden slots, so break/continue leave nothing on the stack
        let keys = self.alloc_slot()?;
        let index = self.alloc_slot()?;
        self.compile_expr(&stmt.right)?;
        self.emit_op(Operation::ForInKeys);
        self.emit_op(Operation::SetLocal(keys));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::LoadFloat64(0.0));
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);

        // the loop variable
        let target: &Pat = match &stmt.left {
            ForHead::VarDecl(var_decl) => {
                if var_decl.kind != VarDeclKind::Var {
                    self.declare_lexical(var_decl)?;
                }
                match var_decl.decls.as_slice() {
                    [decl] if decl.init.is_none() => &decl.name,
                    _ => return unsupported("for-in declaration", var_decl.span),
                }
            }
            ForHead::Pat(pat) => pat,
            ForHead::UsingDecl(using) => return unsupported("using declaration", using.span),
        };

        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::GetLocal(keys));
        self.emit_op(load_str("length"));
        self.emit_op(Operation::GetProperty);
        self.emit_op(Operation::Lt);
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);

        // target = keys[index]
        match target {
            Pat::Ident(ident) => {
                self.emit_op(Operation::GetLocal(keys));
                self.emit_op(Operation::GetLocal(index));
                self.emit_op(Operation::GetProperty);
                self.emit_set_var(&ident.id)?;
            }
            Pat::Expr(expr) if matches!(&**expr, Expr::Member(_)) => {
                let Expr::Member(member_expr) = &**expr else {
                    unreachable!()
                };
                self.compile_expr(&member_expr.obj)?;
                self.compile_member_prop(&member_expr.prop)?;
                self.emit_op(Operation::GetLocal(keys));
                self.emit_op(Operation::GetLocal(index));
                self.emit_op(Operation::GetProperty);
                self.emit_op(Operation::SetProperty);
            }
            pat => return unsupported("for-in target", pat.span()),
        }
        self.emit_op(Operation::Pop);

        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::LoadFloat64(1.0));
        self.emit_op(Operation::Add);
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    fn compile_labeled(&mut self, stmt: &LabeledStmt) -> CompileResult {
        self.pending_labels.push(stmt.label.sym.to_string());
        match &*stmt.body {
            // these attach the pending labels themselves
            Stmt::While(_)
            | Stmt::DoWhile(_)
            | Stmt::For(_)
            | Stmt::ForIn(_)
            | Stmt::Switch(_)
            | Stmt::Labeled(_) => self.compile_stmt(&stmt.body, false),
            body => {
                self.push_breakable(BreakableKind::Labeled);
                self.compile_stmt(body, false)?;
                self.pop_breakable();
                Ok(())
            }
        }
    }

    fn compile_switch(&mut self, stmt: &SwitchStmt) -> CompileResult {
        // the whole switch body is one block scope
        self.enter_scope();
        let discriminant = self.alloc_slot()?;
        self.compile_expr(&stmt.discriminant)?;
        self.emit_op(Operation::SetLocal(discriminant));
        self.emit_op(Operation::Pop);

        let all_stmts: Vec<&Stmt> = stmt.cases.iter().flat_map(|c| c.cons.iter()).collect();
        self.hoist_declarations(&all_stmts, false)?;

        self.push_breakable(BreakableKind::Switch);

        // tests, in order, each jumping to its case body
        let mut case_jumps = Vec::with_capacity(stmt.cases.len());
        for case in &stmt.cases {
            match &case.test {
                Some(test) => {
                    self.emit_op(Operation::GetLocal(discriminant));
                    self.compile_expr(test)?;
                    self.emit_op(Operation::EqEqEq);
                    let next = self.emit_jump(Operation::JumpIfFalse(0));
                    self.emit_op(Operation::Pop);
                    case_jumps.push(Some(self.emit_jump(Operation::Jump(0))));
                    self.patch_jump(next);
                    self.emit_op(Operation::Pop);
                }
                None => case_jumps.push(None),
            }
        }
        // no case matched: go to `default`, or past the switch
        let default_jump = self.emit_jump(Operation::Jump(0));
        let mut has_default = false;

        // bodies, in order, falling through
        for (case, jump) in stmt.cases.iter().zip(case_jumps) {
            match jump {
                Some(jump) => self.patch_jump(jump),
                None => {
                    self.patch_jump(default_jump);
                    has_default = true;
                }
            }
            for stmt in &case.cons {
                self.compile_stmt(stmt, false)?;
            }
        }
        if !has_default {
            self.breakables.last_mut().unwrap().breaks.push(default_jump);
        }

        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    // Layouts (H = catch handler, FT = finally-on-throw, F = finally block):
    //   try/catch:          TryStart(H) B TryEnd Jump END; H: bind C; END:
    //   try/finally:        TryStart(FT) B TryEnd Jump N; FT: save F rethrow; N: F
    //   try/catch/finally:  TryStart(H) B TryEnd Jump N; H: bind TryStart(FT) C TryEnd Jump N;
    //                       FT: save F rethrow; N: F
    fn compile_try(&mut self, stmt: &TryStmt) -> CompileResult {
        let finally = stmt.finalizer.as_ref();

        let handler = self.emit_jump(Operation::TryStart(0));
        self.push_try_region(finally);
        self.compile_block(&stmt.block)?;
        self.try_stack.pop();
        self.emit_op(Operation::TryEnd);
        let mut to_normal_end = vec![self.emit_jump(Operation::Jump(0))];
        self.patch_jump(handler);

        let mut finally_handler = None;
        match &stmt.handler {
            Some(catch) => {
                // the thrown value is on the stack
                self.enter_scope();
                match &catch.param {
                    Some(Pat::Ident(ident)) => {
                        let slot = self.declare_variable(ident.id.sym.to_string())?;
                        self.emit_op(Operation::SetLocal(slot));
                    }
                    Some(pat) => return unsupported("catch parameter pattern", pat.span()),
                    None => {}
                }
                self.emit_op(Operation::Pop);

                if finally.is_some() {
                    finally_handler = Some(self.emit_jump(Operation::TryStart(0)));
                    self.push_try_region(finally);
                }
                self.compile_block(&catch.body)?;
                if finally.is_some() {
                    self.try_stack.pop();
                    self.emit_op(Operation::TryEnd);
                    to_normal_end.push(self.emit_jump(Operation::Jump(0)));
                }
                self.exit_scope();
            }
            None => {}
        }

        if let Some(finally) = finally {
            // thrown out of the try (or catch): run finally, then rethrow
            if let Some(jump) = finally_handler {
                self.patch_jump(jump);
            }
            let saved = self.alloc_slot()?;
            self.emit_op(Operation::SetLocal(saved));
            self.emit_op(Operation::Pop);
            self.compile_block(finally)?;
            self.emit_op(Operation::GetLocal(saved));
            self.emit_op(Operation::Throw);

            for jump in to_normal_end {
                self.patch_jump(jump);
            }
            self.compile_block(finally)?;
        } else {
            for jump in to_normal_end {
                self.patch_jump(jump);
            }
        }
        Ok(())
    }

    fn push_try_region(&mut self, finally: Option<&BlockStmt>) {
        self.try_stack.push(TryRegion {
            finally: finally.cloned(),
            locals_len: self.locals.len(),
            scope_depth: self.current_scope_depth,
        });
    }

    /// Leave every try region above `depth` (innermost first): drop its handler and run
    /// its `finally` inline. The regions stay on `try_stack`, since only this exit path leaves them.
    fn emit_unwind_tries(&mut self, depth: usize) -> CompileResult {
        let mut i = self.try_stack.len();
        while i > depth {
            i -= 1;
            self.emit_op(Operation::TryEnd);
            let Some(finally) = self.try_stack[i].finally.clone() else {
                continue;
            };

            // compile the finally as if at the `try` statement: outside region i and
            // everything nested in it
            let locals_len = self.try_stack[i].locals_len;
            let scope_depth = self.try_stack[i].scope_depth;
            let inner_tries = self.try_stack.split_off(i);
            let inner_locals = self.locals.split_off(locals_len);
            let inner_depth = std::mem::replace(&mut self.current_scope_depth, scope_depth);
            let first_inner = self
                .breakables
                .iter()
                .position(|b| b.try_depth > i)
                .unwrap_or(self.breakables.len());
            let inner_breakables = self.breakables.split_off(first_inner);

            let result = self.compile_block(&finally);

            self.breakables.extend(inner_breakables);
            self.current_scope_depth = inner_depth;
            self.locals.extend(inner_locals);
            self.try_stack.extend(inner_tries);
            result?;
        }
        Ok(())
    }

    fn push_breakable(&mut self, kind: BreakableKind) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.breakables.push(Breakable {
            kind,
            labels,
            breaks: Vec::new(),
            continues: Vec::new(),
            try_depth: self.try_stack.len(),
        });
    }

    fn patch_continues(&mut self) {
        let continues = std::mem::take(&mut self.breakables.last_mut().unwrap().continues);
        for jump in continues {
            self.patch_jump(jump);
        }
    }

    fn pop_breakable(&mut self) {
        let breakable = self.breakables.pop().unwrap();
        for jump in breakable.breaks {
            self.patch_jump(jump);
        }
    }

    fn find_breakable(&self, label: Option<&str>, is_continue: bool, span: Span) -> CompileResult<usize> {
        let found = self.breakables.iter().rposition(|b| match label {
            Some(label) => b.labels.iter().any(|l| l == label),
            None if is_continue => b.kind == BreakableKind::Loop,
            None => b.kind != BreakableKind::Labeled,
        });
        match found {
            Some(i) if is_continue && self.breakables[i].kind != BreakableKind::Loop => {
                unsupported("continue to a non-loop label", span)
            }
            Some(i) => Ok(i),
            None if is_continue => unsupported("continue outside a loop", span),
            None => unsupported("break outside a loop or switch", span),
        }
    }

    // ---------------------------------------------------------------------------------
    // scopes and variables
    // ---------------------------------------------------------------------------------

    fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    /// Leave a block: its names go out of scope (their slots are not reused).
    fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
        while self
            .locals
            .last()
            .is_some_and(|l| l.depth > self.current_scope_depth)
        {
            self.locals.pop();
        }
    }

    /// A fresh slot with no name, for compiler-internal values.
    fn alloc_slot(&mut self) -> CompileResult<u8> {
        if self.slot_count >= 256 {
            return Err(CompileError::TooManyLocals);
        }
        let slot = self.slot_count as u8;
        self.slot_count += 1;
        Ok(slot)
    }

    /// The function's scratch slot, for values held only between two adjacent instructions.
    fn temp(&mut self) -> CompileResult<u8> {
        match self.temp_slot {
            Some(slot) => Ok(slot),
            None => {
                let slot = self.alloc_slot()?;
                self.temp_slot = Some(slot);
                Ok(slot)
            }
        }
    }

    /// Declare a name in the current block and return its new slot.
    fn declare_variable(&mut self, name: String) -> CompileResult<u8> {
        let slot = self.alloc_slot()?;
        self.locals.push(Local {
            name,
            depth: self.current_scope_depth,
            slot,
        });
        Ok(slot)
    }

    /// Declare a name unless the current block already has it (`var` redeclaring a param, etc).
    fn declare_if_absent(&mut self, name: String) -> CompileResult<u8> {
        let existing = self
            .locals
            .iter()
            .rev()
            .take_while(|l| l.depth == self.current_scope_depth)
            .find(|l| l.name == name);
        match existing {
            Some(local) => Ok(local.slot),
            None => self.declare_variable(name),
        }
    }

    fn resolve_local(&self, name: &str) -> Option<(u8, u8)> {
        if let Some(local) = self.locals.iter().rev().find(|l| l.name == name) {
            return Some((0, local.slot));
        }
        let enclosing = self.enclosing?;
        let (depth, slot) = enclosing.resolve_local(name)?;
        Some((depth + 1, slot))
    }

    /// Push the value of a variable. Undeclared names are an error, except the host's
    /// `window` / `globalThis`.
    fn emit_get_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        match self.resolve_local(name) {
            Some((0, slot)) => self.emit_op(Operation::GetLocal(slot)),
            Some((depth, slot)) => self.emit_op(Operation::GetUpvalue(depth, slot)),
            None => match (HostGlobal::from_name(name), name) {
                (Some(host), _) => self.emit_op(Operation::HostGlobal(host as u8)),
                // read-only global constants compile to plain values
                (None, "undefined") => self.emit_op(Operation::Undefined),
                (None, "NaN") => self.emit_op(Operation::LoadFloat64(f64::NAN)),
                (None, "Infinity") => self.emit_op(Operation::LoadFloat64(f64::INFINITY)),
                (None, "arguments") if self.enclosing.is_some() => {
                    self.emit_op(Operation::Arguments)
                }
                (None, _) => {
                    return unsupported(format!("undeclared variable `{name}`"), ident.span)
                }
            },
        }
        Ok(())
    }

    /// Store the top of the stack into a declared variable, leaving it on the stack.
    fn emit_set_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        match self.resolve_local(name) {
            Some((0, slot)) => self.emit_op(Operation::SetLocal(slot)),
            Some((depth, slot)) => self.emit_op(Operation::SetUpvalue(depth, slot)),
            None if HostGlobal::from_name(name).is_some() => {
                return unsupported(format!("assignment to `{name}`"), ident.span)
            }
            None => {
                return unsupported(
                    format!("assignment to undeclared variable `{name}`"),
                    ident.span,
                )
            }
        }
        Ok(())
    }

    fn compile_var_decl(&mut self, var_decl: &VarDecl) -> CompileResult {
        for decl in var_decl.decls.iter() {
            self.compile_var_declarator(var_decl.kind, decl)?;
        }
        Ok(())
    }

    /// The name was already declared when hoisted; this only assigns it.
    fn compile_var_declarator(
        &mut self,
        kind: VarDeclKind,
        var_declarator: &VarDeclarator,
    ) -> CompileResult {
        let Pat::Ident(name) = &var_declarator.name else {
            return unsupported("destructuring declaration", var_declarator.name.span());
        };
        match var_declarator.init {
            Some(ref init) => self.compile_expr(init)?,
            // `var x;` leaves an existing value alone
            None if kind == VarDeclKind::Var => return Ok(()),
            None => self.emit_op(Operation::Undefined),
        }
        self.emit_set_var(&name.id)?;
        self.emit_op(Operation::Pop);
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // functions
    // ---------------------------------------------------------------------------------

    fn compile_fn_decl(&mut self, fn_decl: &FnDecl, internal: bool) -> CompileResult {
        self.compile_function(Some(&fn_decl.ident), &fn_decl.function, false, internal)?;
        self.emit_set_var(&fn_decl.ident)?;
        self.emit_op(Operation::Pop);
        Ok(())
    }

    /// Push a new function. `self_binding` makes `name` refer to the function inside its
    /// own body (named function expressions). `internal` functions get no host wrapper.
    fn compile_function(
        &mut self,
        name: Option<&Ident>,
        function: &Function,
        self_binding: bool,
        internal: bool,
    ) -> CompileResult {
        let mut fn_compiler = Compiler::new_enclosing(self);

        // parameters are the first slots; the VM fills them from the call's arguments
        for param in &function.params {
            match &param.pat {
                Pat::Ident(ident) => {
                    fn_compiler.declare_variable(ident.id.sym.to_string())?;
                }
                pat => return unsupported("parameter pattern", pat.span()),
            }
        }

        if let (true, Some(name)) = (self_binding, name) {
            // a parameter with the same name wins
            if !fn_compiler.locals.iter().any(|l| l.name == *name.sym) {
                let slot = fn_compiler.declare_variable(name.sym.to_string())?;
                fn_compiler.emit_op(Operation::Callee);
                fn_compiler.emit_op(Operation::SetLocal(slot));
                fn_compiler.emit_op(Operation::Pop);
            }
        }

        if let Some(body) = &function.body {
            let stmts: Vec<&Stmt> = body.stmts.iter().collect();
            fn_compiler.compile_body(&stmts, true, false)?;
        }

        // implicit `return undefined`
        fn_compiler.emit_op(Operation::Undefined);
        fn_compiler.emit_op(Operation::Return);

        let code = fn_compiler.bytecode;
        let arity = function.params.len() as u8;
        // an internal function's name and length can't be observed, so leave the name out
        let name = match name {
            Some(n) if !internal => n.sym.to_string(),
            _ => String::new(),
        };
        self.emit_op(Operation::Function(arity, name, code, internal));
        Ok(())
    }

    fn compile_call(&mut self, expr: &CallExpr) -> CompileResult {
        let callee = match &expr.callee {
            Callee::Expr(callee_expr) => callee_expr,
            Callee::Super(s) => return unsupported("super call", s.span),
            Callee::Import(i) => return unsupported("dynamic import", i.span),
        };

        // `obj.f()` (and `(obj.f)()`) call f with obj as `this`
        let mut unwrapped = &**callee;
        while let Expr::Paren(paren) = unwrapped {
            unwrapped = &paren.expr;
        }
        let is_method = if let Expr::Fn(fn_expr) = unwrapped {
            // IIFE `(function () {...})()`: internal unless its own name escapes in its body
            let internal = match &fn_expr.ident {
                Some(ident) => !fn_escapes_in_function(&ident.sym, &fn_expr.function),
                None => true,
            };
            self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, internal)?;
            false
        } else if let Expr::Member(member_expr) = unwrapped {
            // stack: [obj, f]
            self.compile_expr(&member_expr.obj)?;
            self.emit_op(Operation::Dup);
            self.compile_member_prop(&member_expr.prop)?;
            self.emit_op(Operation::GetProperty);
            true
        } else {
            self.compile_expr(callee)?;
            false
        };

        self.compile_args(&expr.args)?;
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
        Ok(())
    }

    /// Push each argument, then the argument count.
    fn compile_args(&mut self, args: &[swc_ecma_ast::ExprOrSpread]) -> CompileResult {
        for arg in args {
            if let Some(spread) = arg.spread {
                return unsupported("spread argument", spread);
            }
            self.compile_expr(&arg.expr)?;
        }
        self.emit_op(Operation::UInt64(args.len() as u64));
        Ok(())
    }

    fn compile_new(&mut self, expr: &NewExpr) -> CompileResult {
        self.compile_expr(&expr.callee)?;
        self.compile_args(expr.args.as_deref().unwrap_or_default())?;
        self.emit_op(Operation::New);
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // expressions
    // ---------------------------------------------------------------------------------

    /// Push the property key of a member expression.
    fn compile_member_prop(&mut self, prop: &MemberProp) -> CompileResult {
        match prop {
            MemberProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            MemberProp::Computed(computed) => self.compile_expr(&computed.expr)?,
            MemberProp::PrivateName(p) => return unsupported("private name", p.span),
        }
        Ok(())
    }

    fn compile_member_expr(&mut self, member_expr: &MemberExpr) -> CompileResult {
        self.compile_expr(&member_expr.obj)?;
        self.compile_member_prop(&member_expr.prop)?;
        self.emit_op(Operation::GetProperty);
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> CompileResult {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit)?,
            Expr::Call(call_expr) => self.compile_call(call_expr)?,
            Expr::New(new_expr) => self.compile_new(new_expr)?,
            Expr::Member(member_expr) => self.compile_member_expr(member_expr)?,
            Expr::Ident(ident) => self.emit_get_var(ident)?,
            Expr::This(_) => self.emit_op(Operation::This),
            Expr::Bin(bin_expr) => self.compile_bin_expr(bin_expr)?,
            Expr::Unary(unary_expr) => self.compile_unary_expr(unary_expr)?,
            Expr::Update(update_expr) => self.compile_update_expr(update_expr)?,
            Expr::Paren(paren_expr) => self.compile_expr(&paren_expr.expr)?,
            Expr::Assign(assign_expr) => self.compile_assign_expr(assign_expr)?,
            Expr::Array(array_lit) => self.compile_array_lit(array_lit)?,
            Expr::Object(object_lit) => self.compile_object_lit(object_lit)?,
            Expr::Cond(cond_expr) => self.compile_cond_expr(cond_expr)?,
            Expr::Seq(seq_expr) => {
                let last = seq_expr.exprs.len() - 1;
                for (i, expr) in seq_expr.exprs.iter().enumerate() {
                    self.compile_expr(expr)?;
                    if i != last {
                        self.emit_op(Operation::Pop);
                    }
                }
            }
            Expr::Fn(fn_expr) => {
                self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, false)?
            }
            _ => return unsupported(expr_kind(expr), expr.span()),
        }
        Ok(())
    }

    fn compile_cond_expr(&mut self, expr: &CondExpr) -> CompileResult {
        self.compile_expr(&expr.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.cons)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.alt)?;
        self.patch_jump(end_jump);
        Ok(())
    }

    fn compile_update_expr(&mut self, expr: &UpdateExpr) -> CompileResult {
        let step = match expr.op {
            UpdateOp::PlusPlus => Operation::Add,
            UpdateOp::MinusMinus => Operation::Sub,
        };
        let mut arg = &*expr.arg;
        while let Expr::Paren(paren) = arg {
            arg = &paren.expr;
        }
        match arg {
            Expr::Ident(ident) => {
                self.emit_get_var(ident)?;
                self.emit_op(Operation::UnaryPlus);
                if !expr.prefix {
                    // keep the old (numeric) value as the result
                    self.emit_op(Operation::Dup);
                }
                self.emit_op(Operation::LoadFloat64(1.0));
                self.emit_op(step);
                self.emit_set_var(ident)?;
                if !expr.prefix {
                    self.emit_op(Operation::Pop);
                }
            }
            Expr::Member(member_expr) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                self.compile_member_prop(&member_expr.prop)?;
                self.emit_op(Operation::Dup2);
                self.emit_op(Operation::GetProperty);
                self.emit_op(Operation::UnaryPlus);
                let temp = if expr.prefix { None } else { Some(self.temp()?) };
                if let Some(temp) = temp {
                    self.emit_op(Operation::SetLocal(temp));
                }
                self.emit_op(Operation::LoadFloat64(1.0));
                self.emit_op(step);
                self.emit_op(Operation::SetProperty);
                if let Some(temp) = temp {
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                }
            }
            _ => return unsupported("update target", expr.arg.span()),
        }
        Ok(())
    }

    fn compile_assign_expr(&mut self, expr: &AssignExpr) -> CompileResult {
        if expr.op != AssignOp::Assign {
            return self.compile_compound_assign(expr);
        }
        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // stack: [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                self.compile_member_prop(&member_expr.prop)?;
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::SetProperty);
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                self.compile_expr(&expr.right)?;
                self.emit_set_var(&ident.id)?;
            }
            AssignTarget::Pat(AssignTargetPat::Array(array_pat)) => {
                self.compile_expr(&expr.right)?;
                self.compile_array_destructuring(array_pat)?;
            }
            target => return unsupported("assignment target", target.span()),
        }
        Ok(())
    }

    fn compile_compound_assign(&mut self, expr: &AssignExpr) -> CompileResult {
        // `&&=`, `||=` and `??=` only evaluate and assign the RHS when needed
        let short_circuit = match expr.op {
            AssignOp::AndAssign => Some(Operation::JumpIfFalse(0)),
            AssignOp::OrAssign => Some(Operation::JumpIfTrue(0)),
            AssignOp::NullishAssign => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        let arithmetic = match expr.op {
            AssignOp::AddAssign => Some(Operation::Add),
            AssignOp::SubAssign => Some(Operation::Sub),
            AssignOp::MulAssign => Some(Operation::Mul),
            AssignOp::DivAssign => Some(Operation::Div),
            AssignOp::ModAssign => Some(Operation::Mod),
            AssignOp::LShiftAssign => Some(Operation::LShift),
            AssignOp::RShiftAssign => Some(Operation::RShift),
            AssignOp::ZeroFillRShiftAssign => Some(Operation::ZeroFillRShift),
            AssignOp::BitOrAssign => Some(Operation::BitOr),
            AssignOp::BitXorAssign => Some(Operation::BitXor),
            AssignOp::BitAndAssign => Some(Operation::BitAnd),
            AssignOp::ExpAssign => Some(Operation::Exp),
            _ => None,
        };

        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                let ident = &ident.id;
                self.emit_get_var(ident)?;
                if let Some(jump_op) = short_circuit {
                    let end = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_set_var(ident)?;
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_set_var(ident)?;
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                self.compile_member_prop(&member_expr.prop)?;
                self.emit_op(Operation::Dup2);
                self.emit_op(Operation::GetProperty);
                if let Some(jump_op) = short_circuit {
                    let keep = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_op(Operation::SetProperty);
                    let end = self.emit_jump(Operation::Jump(0));
                    // not assigning: drop [obj, key] from under the value
                    self.patch_jump(keep);
                    let temp = self.temp()?;
                    self.emit_op(Operation::SetLocal(temp));
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_op(Operation::SetProperty);
                }
            }
            target => return unsupported("compound assignment target", target.span()),
        }
        Ok(())
    }

    fn compile_array_lit(&mut self, array_lit: &ArrayLit) -> CompileResult {
        for elem in &array_lit.elems {
            match elem {
                Some(expr_or_spread) => {
                    if let Some(spread) = expr_or_spread.spread {
                        return unsupported("spread element", spread);
                    }
                    self.compile_expr(&expr_or_spread.expr)?;
                }
                None => self.emit_op(Operation::Hole),
            }
        }
        self.emit_op(Operation::Array(array_lit.elems.len() as u32));
        Ok(())
    }

    fn compile_object_lit(&mut self, object_lit: &ObjectLit) -> CompileResult {
        for prop in &object_lit.props {
            let PropOrSpread::Prop(prop) = prop else {
                return unsupported("object spread", prop.span());
            };
            let Prop::KeyValue(key_value) = &**prop else {
                return unsupported("object property kind", prop.span());
            };
            match &key_value.key {
                PropName::Ident(ident) => self.emit_op(load_str(&ident.sym)),
                PropName::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
                // the VM converts the number to a key the same way JS does
                PropName::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
                PropName::Computed(computed) => self.compile_expr(&computed.expr)?,
                key => return unsupported("object key", key.span()),
            }
            self.compile_expr(&key_value.value)?;
        }
        self.emit_op(Operation::Object(object_lit.props.len() as u32));
        Ok(())
    }

    fn compile_array_destructuring(&mut self, array_pat: &ArrayPat) -> CompileResult {
        // the value to destructure is on top of the stack
        for (i, elem) in array_pat.elems.iter().enumerate() {
            let Some(pat) = elem else { continue };
            let Pat::Ident(ident) = pat else {
                return unsupported("destructuring pattern", pat.span());
            };
            let is_last = i == array_pat.elems.len() - 1;

            // keep the array around for the next element
            if !is_last {
                self.emit_op(Operation::Dup);
            }
            self.emit_op(Operation::LoadFloat64(i as f64));
            self.emit_op(Operation::GetProperty);
            self.emit_set_var(&ident.id)?;
            if !is_last {
                self.emit_op(Operation::Pop);
            }
        }
        Ok(())
    }

    fn compile_bin_expr(&mut self, expr: &BinExpr) -> CompileResult {
        // `&&`, `||` and `??` only evaluate the right side when needed
        let short_circuit = match expr.op {
            BinaryOp::LogicalAnd => Some(Operation::JumpIfFalse(0)),
            BinaryOp::LogicalOr => Some(Operation::JumpIfTrue(0)),
            BinaryOp::NullishCoalescing => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        if let Some(jump_op) = short_circuit {
            self.compile_expr(&expr.left)?;
            let end = self.emit_jump(jump_op);
            self.emit_op(Operation::Pop);
            self.compile_expr(&expr.right)?;
            self.patch_jump(end);
            return Ok(());
        }

        self.compile_expr(&expr.left)?;
        self.compile_expr(&expr.right)?;
        let op = match expr.op {
            BinaryOp::Add => Operation::Add,
            BinaryOp::Sub => Operation::Sub,
            BinaryOp::Mul => Operation::Mul,
            BinaryOp::Div => Operation::Div,
            BinaryOp::Mod => Operation::Mod,
            BinaryOp::EqEq => Operation::EqEq,
            BinaryOp::NotEq => Operation::NotEq,
            BinaryOp::EqEqEq => Operation::EqEqEq,
            BinaryOp::NotEqEq => Operation::NotEqEq,
            BinaryOp::Lt => Operation::Lt,
            BinaryOp::LtEq => Operation::LtEq,
            BinaryOp::Gt => Operation::Gt,
            BinaryOp::GtEq => Operation::GtEq,
            BinaryOp::LShift => Operation::LShift,
            BinaryOp::RShift => Operation::RShift,
            BinaryOp::ZeroFillRShift => Operation::ZeroFillRShift,
            BinaryOp::BitOr => Operation::BitOr,
            BinaryOp::BitXor => Operation::BitXor,
            BinaryOp::BitAnd => Operation::BitAnd,
            BinaryOp::In => Operation::In,
            BinaryOp::InstanceOf => Operation::InstanceOf,
            BinaryOp::Exp => Operation::Exp,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing => {
                unreachable!("handled above")
            }
        };
        self.emit_op(op);
        Ok(())
    }

    fn compile_unary_expr(&mut self, expr: &UnaryExpr) -> CompileResult {
        if expr.op == UnaryOp::Delete {
            // `delete obj.key` needs the object and key, not the property value
            let Expr::Member(member_expr) = &*expr.arg else {
                return unsupported("delete of non-member expression", expr.span);
            };
            self.compile_expr(&member_expr.obj)?;
            self.compile_member_prop(&member_expr.prop)?;
            self.emit_op(Operation::Delete);
            return Ok(());
        }

        // `typeof window` must not throw when the host has no `window`
        if expr.op == UnaryOp::TypeOf {
            if let Expr::Ident(ident) = &*expr.arg {
                if let (None, Some(host)) = (
                    self.resolve_local(&ident.sym),
                    HostGlobal::from_name(&ident.sym),
                ) {
                    self.emit_op(Operation::HostGlobal(host as u8 | HOST_GLOBAL_TYPEOF));
                    self.emit_op(Operation::TypeOf);
                    return Ok(());
                }
            }
        }

        self.compile_expr(&expr.arg)?;
        let op = match expr.op {
            UnaryOp::Minus => Operation::UnaryMinus,
            UnaryOp::Plus => Operation::UnaryPlus,
            UnaryOp::Bang => Operation::LogicalNot,
            UnaryOp::Tilde => Operation::BitwiseNot,
            UnaryOp::TypeOf => Operation::TypeOf,
            UnaryOp::Void => Operation::Void,
            UnaryOp::Delete => unreachable!("handled above"),
        };
        self.emit_op(op);
        Ok(())
    }

    fn compile_lit(&mut self, lit: &Lit) -> CompileResult {
        match lit {
            Lit::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            Lit::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            Lit::Bool(b) => self.emit_op(Operation::Bool(b.value)),
            Lit::Null(_) => self.emit_op(Operation::Null),
            Lit::Regex(r) => {
                self.emit_op(Operation::Regex(r.exp.to_string(), r.flags.to_string()));
            }
            Lit::BigInt(b) => return unsupported("BigInt literal", b.span),
            Lit::JSXText(j) => return unsupported("JSX", j.span),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // emitting
    // ---------------------------------------------------------------------------------

    /// Emit a forward jump with a placeholder offset; returns the offset's position for `patch_jump`.
    fn emit_jump(&mut self, op: Operation) -> usize {
        self.emit_op(op);
        self.bytecode.len() - 4
    }

    /// Point a jump emitted by `emit_jump` at the current end of the bytecode.
    fn patch_jump(&mut self, pos: usize) {
        let offset = (self.bytecode.len() - (pos + 4)) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// Emit a backward jump to `start`.
    fn emit_loop(&mut self, start: usize) {
        // the VM lands on `start` after reading the 4 byte operand
        let offset = (self.bytecode.len() + 1 + 4 - start) as u32;
        self.emit_op(Operation::Loop(offset));
    }

    fn emit_op(&mut self, op: Operation) {
        self.bytecode.push(op.get_opcode());
        match op {
            Operation::LoadString(bytes) => self.emit_string(&bytes),
            Operation::LoadFloat64(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Bool(b) => self.bytecode.push(b as u8),
            Operation::Regex(exp, flags) => {
                self.emit_string(exp.as_bytes());
                self.emit_string(flags.as_bytes());
            }
            Operation::UInt8(n) => self.bytecode.push(n),
            Operation::UInt16(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt32(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt64(n) => {
                // can't easily emit 64-bit integers in JS, so we split into two 32-bit integers
                let low = n as u32;
                let high = (n >> 32) as u32;
                self.bytecode.extend_from_slice(&low.to_le_bytes());
                self.bytecode.extend_from_slice(&high.to_le_bytes());
            }
            Operation::Function(arity, name, code, _) => {
                self.bytecode.push(arity);
                self.emit_string(name.as_bytes());
                let code_len = code.len() as u32;
                self.bytecode.extend_from_slice(&code_len.to_le_bytes());
                self.bytecode.extend_from_slice(&code);
            }
            Operation::GetUpvalue(depth, index) | Operation::SetUpvalue(depth, index) => {
                self.bytecode.push(depth);
                self.bytecode.push(index);
            }
            Operation::HostGlobal(id) => self.bytecode.push(id),
            Operation::GetLocal(index) | Operation::SetLocal(index) => self.bytecode.push(index),
            Operation::Array(n) | Operation::Object(n) => {
                self.bytecode.extend_from_slice(&n.to_le_bytes())
            }
            Operation::Jump(offset)
            | Operation::JumpIfFalse(offset)
            | Operation::JumpIfTrue(offset)
            | Operation::JumpIfNotNullish(offset)
            | Operation::Loop(offset)
            | Operation::TryStart(offset) => {
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            _ => {}
        }
    }

    /// Strings are a u32 byte length followed by WTF-8 bytes.
    fn emit_string(&mut self, bytes: &[u8]) {
        self.bytecode
            .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.bytecode.extend_from_slice(bytes);
    }
}

impl Default for Compiler<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::ForOf(_) => "for-of loop",
        Stmt::With(_) => "with statement",
        Stmt::Decl(Decl::Class(_)) => "class declaration",
        Stmt::Decl(_) => "declaration",
        _ => "statement",
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Arrow(_) => "arrow function",
        Expr::Class(_) => "class expression",
        Expr::Tpl(_) | Expr::TaggedTpl(_) => "template literal",
        Expr::Yield(_) => "yield",
        Expr::Await(_) => "await",
        Expr::OptChain(_) => "optional chaining",
        Expr::SuperProp(_) => "super property",
        Expr::MetaProp(_) => "meta property",
        _ => "expression",
    }
}

/// Collects `var` names declared anywhere in a function body, without entering nested functions.
struct VarCollector<'n> {
    names: &'n mut Vec<String>,
}

impl Visit for VarCollector<'_> {
    fn visit_var_decl(&mut self, n: &VarDecl) {
        if n.kind == VarDeclKind::Var {
            for decl in &n.decls {
                if let Pat::Ident(ident) = &decl.name {
                    self.names.push(ident.id.sym.to_string());
                }
            }
        }
        n.visit_children_with(self);
    }
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Whether the function named `name` could be seen by anything other than a direct call
/// `name(...)` within `stmts`. Only directly-called functions can skip the host wrapper:
/// any other read (`g = f`, `arr.map(f)`, `window.x = f`, `typeof f`, `new f`, `{ f }`)
/// lets the value leave VM control. Shadowed names in nested scopes are counted too, which
/// can only make the answer more cautious.
fn fn_escapes(name: &str, stmts: &[&Stmt]) -> bool {
    let mut scan = EscapeScan { name, escapes: false };
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.escapes
}

fn fn_escapes_in_function(name: &str, function: &Function) -> bool {
    let mut scan = EscapeScan { name, escapes: false };
    function.visit_with(&mut scan);
    scan.escapes
}

struct EscapeScan<'n> {
    name: &'n str,
    escapes: bool,
}

impl EscapeScan<'_> {
    fn is_name(&self, expr: &Expr) -> bool {
        let mut expr = expr;
        while let Expr::Paren(paren) = expr {
            expr = &paren.expr;
        }
        matches!(expr, Expr::Ident(ident) if *ident.sym == *self.name)
    }
}

impl Visit for EscapeScan<'_> {
    fn visit_call_expr(&mut self, n: &CallExpr) {
        match &n.callee {
            // a direct call doesn't let the function out; its arguments still might
            Callee::Expr(callee) if self.is_name(callee) => n.args.visit_with(self),
            _ => n.visit_children_with(self),
        }
    }
    fn visit_expr(&mut self, n: &Expr) {
        if self.is_name(n) {
            self.escapes = true;
        }
        n.visit_children_with(self);
    }
    fn visit_prop(&mut self, n: &Prop) {
        if let Prop::Shorthand(ident) = n {
            if *ident.sym == *self.name {
                self.escapes = true;
            }
        }
        n.visit_children_with(self);
    }
}
