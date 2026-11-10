use std::collections::HashMap;
use std::path::Path;
use swc_common::sync::Lrc;
use swc_common::{SourceFile, SourceMap, Span, Spanned};
use swc_ecma_ast::{
    ArrayLit, ArrayPat, AssignExpr, AssignOp, AssignTarget, AssignTargetPat, BinExpr, BinaryOp,
    CallExpr, Callee, Decl, Expr, FnDecl, Lit, MemberExpr, MemberProp, Module, ModuleItem,
    ObjectLit, Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp,
    VarDecl, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use thiserror::Error;

pub struct Compiler<'a> {
    bytecode: Vec<u8>,
    local_count: usize,
    locals: [Option<Local>; 256],
    current_scope_depth: usize,
    scope: HashMap<String, usize>,
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

struct VirtualFunction {
    name: String,
    arity: usize,
    f_type: VirtualFunctionType,
}

enum VirtualFunctionType {
    Function,
    Script,
}

struct Local {
    name: String,
    depth: usize,
}

#[derive(Debug)]
enum Operation {
    Pop,
    Return,
    LoadFloat64(f64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    /// "kumo"
    LoadString(String),
    /// true
    Bool(bool),
    /// null
    Null,
    /// `undefined`
    Undefined,
    ///
    Regex(String, String),
    /// x = 1
    StoreVar(String),
    /// x
    LoadVar(String),
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
    /// Create a virtual function: arity, name_len, name, code_len, code
    Function(u8, String, Vec<u8>),
    /// Get variable from enclosing scope: depth, index
    GetUpvalue(u8, u8),
    /// Set variable in enclosing scope: depth, index
    SetUpvalue(u8, u8),
    /// Jump forward by offset
    Jump(u32),
    /// Jump forward by offset if top of stack is falsy (does not pop)
    JumpIfFalse(u32),
    /// Jump forward by offset if top of stack is truthy (does not pop)
    JumpIfTrue(u32),
    /// Jump forward by offset if top of stack is not null/undefined (does not pop)
    JumpIfNotNullish(u32),
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
            Operation::StoreVar(_) => 0x09,
            Operation::LoadVar(_) => 0x0a,
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
            Operation::Function(_, _, _) => 0x37,
            Operation::GetUpvalue(_, _) => 0x38,
            Operation::SetUpvalue(_, _) => 0x39,
            Operation::Jump(_) => 0x3a,
            Operation::JumpIfFalse(_) => 0x3b,
            Operation::JumpIfTrue(_) => 0x3c,
            Operation::JumpIfNotNullish(_) => 0x3d,
        }
    }

    fn get_name(&self) -> &'static str {
        match self {
            Operation::Return => "OP_RETURN",
            Operation::LoadString(_) => "OP_LOAD_STRING",
            Operation::LoadFloat64(_) => "OP_LOAD_FLOAT64",
            Operation::Bool(_) => "OP_LOAD_BOOL",
            Operation::Pop => "OP_POP",
            Operation::Null => "OP_NULL",
            Operation::Undefined => "OP_UNDEFINED",
            Operation::Regex(_, _) => "OP_REGEX",
            Operation::StoreVar(_) => "OP_STORE_VAR",
            Operation::LoadVar(_) => "OP_LOAD_VAR",
            Operation::UInt8(_) => "OP_UINT_8",
            Operation::UInt16(_) => "OP_UINT_16",
            Operation::UInt32(_) => "OP_UINT_32",
            Operation::UInt64(_) => "OP_UINT_64",
            Operation::Call => "OP_CALL",
            Operation::GetProperty => "OP_GET_PROPERTY",
            Operation::SetProperty => "OP_SET_PROPERTY",
            Operation::EqEq => "OP_EQ_EQ",
            Operation::NotEq => "OP_NOT_EQ",
            Operation::EqEqEq => "OP_EQ_EQ_EQ",
            Operation::NotEqEq => "OP_NOT_EQ_EQ",
            Operation::Lt => "OP_LT",
            Operation::LtEq => "OP_LT_EQ",
            Operation::Gt => "OP_GT",
            Operation::GtEq => "OP_GT_EQ",
            Operation::LShift => "OP_LSHIFT",
            Operation::RShift => "OP_RSHIFT",
            Operation::ZeroFillRShift => "OP_ZERO_FILL_RSHIFT",
            Operation::Add => "OP_ADD",
            Operation::Sub => "OP_SUB",
            Operation::Mul => "OP_MUL",
            Operation::Div => "OP_DIV",
            Operation::Mod => "OP_MOD",
            Operation::BitOr => "OP_BIT_OR",
            Operation::BitXor => "OP_BIT_XOR",
            Operation::BitAnd => "OP_BIT_AND",
            Operation::In => "OP_IN",
            Operation::InstanceOf => "OP_INSTANCE_OF",
            Operation::Exp => "OP_EXP",
            Operation::GetLocal(_) => "OP_GET_LOCAL",
            Operation::SetLocal(_) => "OP_SET_LOCAL",
            Operation::UnaryMinus => "OP_UNARY_MINUS",
            Operation::UnaryPlus => "OP_UNARY_PLUS",
            Operation::LogicalNot => "OP_LOGICAL_NOT",
            Operation::BitwiseNot => "OP_BITWISE_NOT",
            Operation::TypeOf => "OP_TYPEOF",
            Operation::Void => "OP_VOID",
            Operation::Delete => "OP_DELETE",
            Operation::Array(_) => "OP_ARRAY",
            Operation::Object(_) => "OP_OBJECT",
            Operation::Dup => "OP_DUP",
            Operation::Function(_, _, _) => "OP_FUNCTION",
            Operation::GetUpvalue(_, _) => "OP_GET_UPVALUE",
            Operation::SetUpvalue(_, _) => "OP_SET_UPVALUE",
            Operation::Jump(_) => "OP_JUMP",
            Operation::JumpIfFalse(_) => "OP_JUMP_IF_FALSE",
            Operation::JumpIfTrue(_) => "OP_JUMP_IF_TRUE",
            Operation::JumpIfNotNullish(_) => "OP_JUMP_IF_NOT_NULLISH",
        }
    }
}

const LOCAL_REPEAT_VALUE: Option<Local> = None;

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Compiler {
            bytecode: Vec::new(),
            local_count: 0,
            locals: [LOCAL_REPEAT_VALUE; 256],
            scope: HashMap::new(),
            current_scope_depth: 0,
            enclosing: None,
        }
    }

    fn new_enclosing(enclosing: &'a Compiler<'a>) -> Compiler<'a> {
        Compiler {
            bytecode: Vec::new(),
            scope: HashMap::new(),
            local_count: 0,
            locals: [LOCAL_REPEAT_VALUE; 256],
            current_scope_depth: 0,
            enclosing: Some(enclosing),
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
        let body_len = module.body.len();
        for (i, item) in module.body.iter().enumerate() {
            let is_last = i == body_len - 1;
            match item {
                ModuleItem::Stmt(stmt) => self.compile_stmt(stmt, is_last)?,
                ModuleItem::ModuleDecl(decl) => return unsupported("import/export", decl.span()),
            }
        }
        Ok(self.bytecode.clone())
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
                self.emit_op(Operation::Return);
            }
            Stmt::Decl(Decl::Fn(fn_decl)) => self.compile_fn_decl(fn_decl)?,
            Stmt::Decl(Decl::Var(var_decl)) => self.compile_var_decl(var_decl)?,
            Stmt::Block(block_stmt) => {
                for stmt in &block_stmt.stmts {
                    self.compile_stmt(stmt, false)?;
                }
            }
            Stmt::Empty(_) => {}
            _ => return unsupported(stmt_kind(stmt), stmt.span()),
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    #[allow(dead_code)]
    fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
    }

    fn add_local(&mut self, name: String, depth: usize) -> CompileResult {
        if self.local_count >= 256 {
            return Err(CompileError::TooManyLocals);
        }
        self.locals[self.local_count] = Some(Local { name, depth });
        self.local_count += 1;
        Ok(())
    }

    /// Declare a local in the current function and return its slot.
    fn declare_variable(&mut self, name: String) -> CompileResult<u8> {
        self.add_local(name, self.current_scope_depth)?;
        Ok((self.local_count - 1) as u8)
    }

    fn resolve_local(&self, name: &str) -> Option<(u8, u8)> {
        // search locals backwards (most recent first)
        for i in (0..self.local_count).rev() {
            if let Some(ref local) = self.locals[i] {
                if local.name == name {
                    return Some((0, i as u8));
                }
            }
        }

        if let Some(enclosing) = self.enclosing {
            if let Some((depth, index)) = enclosing.resolve_local(name) {
                return Some((depth + 1, index));
            }
        }

        None
    }

    /// Push the value of a variable (local, upvalue or global).
    fn emit_get_var(&mut self, name: String) {
        match self.resolve_local(&name) {
            Some((0, index)) => self.emit_op(Operation::GetLocal(index)),
            Some((depth, index)) => self.emit_op(Operation::GetUpvalue(depth, index)),
            None => self.emit_op(Operation::LoadVar(name)),
        }
    }

    /// Store the top of the stack into a variable, leaving it on the stack.
    fn emit_set_var(&mut self, name: String) {
        match self.resolve_local(&name) {
            Some((0, index)) => self.emit_op(Operation::SetLocal(index)),
            Some((depth, index)) => self.emit_op(Operation::SetUpvalue(depth, index)),
            None => self.emit_op(Operation::StoreVar(name)),
        }
    }

    fn compile_var_decl(&mut self, var_decl: &VarDecl) -> CompileResult {
        for decl in var_decl.decls.iter() {
            self.compile_var_declarator(decl)?;
        }
        Ok(())
    }

    fn compile_fn_decl(&mut self, fn_decl: &FnDecl) -> CompileResult {
        let name = fn_decl.ident.sym.to_string();
        let function = &fn_decl.function;

        // compile function body in a new compiler instance
        let mut fn_compiler = Compiler::new_enclosing(self);

        // parameters are the first locals
        for param in &function.params {
            match &param.pat {
                Pat::Ident(ident) => {
                    fn_compiler.declare_variable(ident.id.sym.to_string())?;
                }
                pat => return unsupported("parameter pattern", pat.span()),
            }
        }

        if let Some(body) = &function.body {
            for stmt in &body.stmts {
                fn_compiler.compile_stmt(stmt, false)?;
            }
        }

        // implicit `return undefined`
        fn_compiler.emit_op(Operation::Undefined);
        fn_compiler.emit_op(Operation::Return);

        let code = fn_compiler.bytecode;
        let arity = function.params.len() as u8;

        self.emit_op(Operation::Function(arity, name.clone(), code));
        let slot = self.declare_variable(name)?;
        self.emit_op(Operation::SetLocal(slot));
        self.emit_op(Operation::Pop);
        Ok(())
    }

    fn compile_var_declarator(&mut self, var_declarator: &VarDeclarator) -> CompileResult {
        let Pat::Ident(name) = &var_declarator.name else {
            return unsupported("destructuring declaration", var_declarator.name.span());
        };
        match var_declarator.init {
            Some(ref init) => self.compile_expr(init)?,
            None => self.emit_op(Operation::Undefined),
        }
        let slot = self.declare_variable(name.id.sym.to_string())?;
        self.emit_op(Operation::SetLocal(slot));
        self.emit_op(Operation::Pop);
        Ok(())
    }

    fn compile_call(&mut self, expr: &CallExpr) -> CompileResult {
        // push function onto stack
        match &expr.callee {
            Callee::Expr(callee_expr) => self.compile_expr(callee_expr)?,
            Callee::Super(s) => return unsupported("super call", s.span),
            Callee::Import(i) => return unsupported("dynamic import", i.span),
        }

        // push args onto stack
        for arg in &expr.args {
            if let Some(spread) = arg.spread {
                return unsupported("spread argument", spread);
            }
            self.compile_expr(&arg.expr)?;
        }
        // push number of args onto stack
        self.emit_op(Operation::UInt64(expr.args.len() as u64));
        self.emit_op(Operation::Call);
        Ok(())
    }

    /// Push the property key of a member expression.
    fn compile_member_prop(&mut self, prop: &MemberProp) -> CompileResult {
        match prop {
            MemberProp::Ident(ident) => self.emit_op(Operation::LoadString(ident.sym.to_string())),
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
            Expr::Member(member_expr) => self.compile_member_expr(member_expr)?,
            Expr::Ident(ident) => self.emit_get_var(ident.sym.to_string()),
            Expr::Bin(bin_expr) => self.compile_bin_expr(bin_expr)?,
            Expr::Unary(unary_expr) => self.compile_unary_expr(unary_expr)?,
            Expr::Paren(paren_expr) => self.compile_expr(&paren_expr.expr)?,
            Expr::Assign(assign_expr) => self.compile_assign_expr(assign_expr)?,
            Expr::Array(array_lit) => self.compile_array_lit(array_lit)?,
            Expr::Object(object_lit) => self.compile_object_lit(object_lit)?,
            _ => return unsupported(expr_kind(expr), expr.span()),
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
                self.emit_set_var(ident.id.sym.to_string());
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
        let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &expr.left else {
            return unsupported("compound assignment target", expr.left.span());
        };
        let name = ident.id.sym.to_string();

        self.emit_get_var(name.clone());

        // `&&=`, `||=` and `??=` only evaluate and assign the RHS when needed
        let short_circuit = match expr.op {
            AssignOp::AndAssign => Some(Operation::JumpIfFalse(0)),
            AssignOp::OrAssign => Some(Operation::JumpIfTrue(0)),
            AssignOp::NullishAssign => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        if let Some(jump_op) = short_circuit {
            let end = self.emit_jump(jump_op);
            self.emit_op(Operation::Pop);
            self.compile_expr(&expr.right)?;
            self.emit_set_var(name);
            self.patch_jump(end);
            return Ok(());
        }

        self.compile_expr(&expr.right)?;
        let op = match expr.op {
            AssignOp::AddAssign => Operation::Add,
            AssignOp::SubAssign => Operation::Sub,
            AssignOp::MulAssign => Operation::Mul,
            AssignOp::DivAssign => Operation::Div,
            AssignOp::ModAssign => Operation::Mod,
            AssignOp::LShiftAssign => Operation::LShift,
            AssignOp::RShiftAssign => Operation::RShift,
            AssignOp::ZeroFillRShiftAssign => Operation::ZeroFillRShift,
            AssignOp::BitOrAssign => Operation::BitOr,
            AssignOp::BitXorAssign => Operation::BitXor,
            AssignOp::BitAndAssign => Operation::BitAnd,
            AssignOp::ExpAssign => Operation::Exp,
            AssignOp::Assign
            | AssignOp::AndAssign
            | AssignOp::OrAssign
            | AssignOp::NullishAssign => unreachable!("handled above"),
        };
        self.emit_op(op);
        self.emit_set_var(name);
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
                None => self.emit_op(Operation::Undefined),
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
                PropName::Ident(ident) => {
                    self.emit_op(Operation::LoadString(ident.sym.to_string()))
                }
                PropName::Str(s) => self.emit_op(Operation::LoadString(
                    s.value.to_string_lossy().into_owned(),
                )),
                PropName::Num(n) => self.emit_op(Operation::LoadString(n.value.to_string())),
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
            self.emit_set_var(ident.id.sym.to_string());
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
            Lit::Str(s) => self.emit_op(Operation::LoadString(
                s.value.to_string_lossy().into_owned(),
            )),
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

    fn emit_op(&mut self, op: Operation) {
        self.bytecode.push(op.get_opcode());
        match op {
            Operation::LoadString(s) => self.emit_string(&s),
            Operation::LoadFloat64(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Bool(b) => {
                if b {
                    self.bytecode.push(0x01);
                } else {
                    self.bytecode.push(0x00);
                }
            }
            Operation::StoreVar(name) => self.emit_string(&name),
            Operation::Regex(exp, flags) => {
                self.emit_string(&exp);
                self.emit_string(&flags);
            }
            Operation::UInt8(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::UInt16(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::UInt32(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::UInt64(n) => {
                // can't easily emit 64-bit integers in JS, so we split into two 32-bit integers
                let low = n as u32;
                let high = (n >> 32) as u32;
                let low_bytes = low.to_le_bytes();
                let high_bytes = high.to_le_bytes();
                self.bytecode.extend_from_slice(&low_bytes);
                self.bytecode.extend_from_slice(&high_bytes);
            }
            Operation::Function(arity, name, code) => {
                self.bytecode.push(arity);
                self.emit_string(&name);
                let code_len = code.len() as u32;
                let bytes = code_len.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
                self.bytecode.extend_from_slice(&code);
            }
            Operation::GetUpvalue(depth, index) => {
                self.bytecode.push(depth);
                self.bytecode.push(index);
            }
            Operation::SetUpvalue(depth, index) => {
                self.bytecode.push(depth);
                self.bytecode.push(index);
            }
            Operation::LoadVar(name) => self.emit_string(&name),
            Operation::GetLocal(index) => {
                self.bytecode.push(index);
            }
            Operation::SetLocal(index) => {
                self.bytecode.push(index);
            }
            Operation::Array(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Object(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Jump(offset)
            | Operation::JumpIfFalse(offset)
            | Operation::JumpIfTrue(offset)
            | Operation::JumpIfNotNullish(offset) => {
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            _ => {}
        }
    }

    fn emit_string(&mut self, s: &str) {
        self.bytecode.extend_from_slice(s.as_bytes());
        self.bytecode.push(0);
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::If(_) => "if statement",
        Stmt::While(_) => "while loop",
        Stmt::DoWhile(_) => "do-while loop",
        Stmt::For(_) => "for loop",
        Stmt::ForIn(_) => "for-in loop",
        Stmt::ForOf(_) => "for-of loop",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::Labeled(_) => "labeled statement",
        Stmt::Switch(_) => "switch statement",
        Stmt::Throw(_) => "throw",
        Stmt::Try(_) => "try statement",
        Stmt::With(_) => "with statement",
        Stmt::Debugger(_) => "debugger statement",
        Stmt::Decl(_) => "declaration",
        _ => "statement",
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::This(_) => "this",
        Expr::Fn(_) => "function expression",
        Expr::Arrow(_) => "arrow function",
        Expr::Class(_) => "class expression",
        Expr::Update(_) => "update expression (++/--)",
        Expr::Cond(_) => "conditional expression",
        Expr::Seq(_) => "comma expression",
        Expr::New(_) => "new expression",
        Expr::Tpl(_) | Expr::TaggedTpl(_) => "template literal",
        Expr::Yield(_) => "yield",
        Expr::Await(_) => "await",
        Expr::OptChain(_) => "optional chaining",
        Expr::SuperProp(_) => "super property",
        Expr::MetaProp(_) => "meta property",
        _ => "expression",
    }
}
