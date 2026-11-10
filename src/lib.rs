use std::any::Any;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use swc_common::errors::{ColorConfig, Handler};
use swc_common::sync::Lrc;
use swc_common::SourceMap;
use swc_ecma_ast::{
    ArrayLit, ArrayPat, AssignExpr, AssignOp, AssignTarget, AssignTargetPat, BinExpr, BinaryOp,
    CallExpr, Callee, Expr, FnDecl, Lit, MemberExpr, Module, ModuleItem, ObjectLit, Pat, Prop,
    PropName, PropOrSpread, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp, VarDecl, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};
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
    /// `||`
    LogicalOr,
    /// `&&`
    LogicalAnd,
    /// `in`
    In,
    /// `instanceof`
    InstanceOf,
    /// `**`
    Exp,
    /// `??`
    NullishCoalescing,
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
            Operation::LogicalOr => 0x25,
            Operation::LogicalAnd => 0x26,
            Operation::In => 0x27,
            Operation::InstanceOf => 0x28,
            Operation::Exp => 0x29,
            Operation::NullishCoalescing => 0x2a,
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
            Operation::LogicalOr => "OP_LOGICAL_OR",
            Operation::LogicalAnd => "OP_LOGICAL_AND",
            Operation::In => "OP_IN",
            Operation::InstanceOf => "OP_INSTANCE_OF",
            Operation::Exp => "OP_EXP",
            Operation::NullishCoalescing => "OP_NULLISH_COALESCING",
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

    pub fn compile_file(&mut self, path: &Path) -> Result<Vec<u8>, CompileError> {
        let cm: Lrc<SourceMap> = Default::default();

        let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

        let fm = cm
            .load_file(path)
            .map_err(|e| CompileError::LoadFileError(e.to_string()))?;

        let lexer = Lexer::new(
            Syntax::Es(Default::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );

        let mut parser = Parser::new_from(lexer);

        for e in parser.take_errors() {
            e.into_diagnostic(&handler).emit();
        }

        // parse input js
        let parse_start = Instant::now();

        let module = parser.parse_module().map_err(|e| {
            // Unrecoverable fatal error occurred
            e.into_diagnostic(&handler).emit();
            CompileError::ParseError("failed to parse module".into())
        })?;

        eprintln!("parsing took {:?}", Instant::now() - parse_start);

        Ok(self.compile(&module))
    }

    pub fn compile(&mut self, module: &Module) -> Vec<u8> {
        let compile_start = Instant::now();

        // process each statement in the module body
        let body_len = module.body.len();
        for (i, item) in module.body.iter().enumerate() {
            let is_last = i == body_len - 1;

            match item {
                ModuleItem::Stmt(stmt) => {
                    self.compile_stmt(stmt, is_last);
                }
                ModuleItem::ModuleDecl(_) => {
                    // module declarations like import/export - not supported yet
                }
            }
        }

        eprintln!("compiling took {:?}", Instant::now() - compile_start);
        self.bytecode.clone()
    }

    fn compile_stmt(&mut self, stmt: &Stmt, is_last_in_module: bool) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                // compile the expression
                self.compile_expr(&expr_stmt.expr);
                // pop the result unless it's the last statement in the module
                // (so the test runner can read it)
                if !is_last_in_module {
                    self.emit_op(Operation::Pop);
                }
            }
            Stmt::Return(return_stmt) => {
                if let Some(arg) = &return_stmt.arg {
                    self.compile_expr(arg);
                } else {
                    self.emit_op(Operation::Undefined);
                }
                self.emit_op(Operation::Return);
            }
            Stmt::Decl(decl) => {
                // handle declarations (variable, function, etc.)
                match decl {
                    swc_ecma_ast::Decl::Fn(fn_decl) => {
                        self.compile_fn_decl(fn_decl);
                    }
                    _ => decl.visit_with(self),
                }
            }
            Stmt::Block(block_stmt) => {
                for stmt in &block_stmt.stmts {
                    self.compile_stmt(stmt, false);
                }
            }
            _ => {
                // for other statement types, use the visitor pattern
                stmt.visit_with(self);
            }
        }
    }

    fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
    }

    fn add_local(&mut self, name: String, depth: usize) {
        if self.local_count >= 256 {
            todo!("too many locals, max number of locals supported is 256");
        }
        self.locals[self.local_count as usize] = Some(Local { name, depth });
        self.local_count += 1;
    }

    fn declare_variable(&mut self, name: String) {
        // sll variables are locals in our implementation
        // they live on the stack at the position determined by local_count
        self.add_local(name, self.current_scope_depth)
    }

    fn resolve_local(&self, name: &str) -> Option<(u8, u8)> {
        // search locals array backwards (most recent first)
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

    // compile variable declarations
    fn compile_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in var_decl.decls.iter() {
            self.compile_var_declator(decl);
        }
    }

    fn compile_fn_decl(&mut self, fn_decl: &FnDecl) {
        let name = fn_decl.ident.sym.to_string();

        // compile function body in a new compiler instance
        let mut fn_compiler = Compiler::new_enclosing(self);

        // add parameters as locals
        for param in &fn_decl.function.params {
            match &param.pat {
                Pat::Ident(ident) => {
                    fn_compiler.declare_variable(ident.id.sym.to_string());
                }
                _ => unimplemented!("Complex parameters not supported"),
            }
        }

        // compile body
        if let Some(body) = &fn_decl.function.body {
            for stmt in &body.stmts {
                fn_compiler.compile_stmt(stmt, false);
            }
        }

        // ensure implicit return undefined if no return statement
        fn_compiler.emit_op(Operation::Undefined);
        fn_compiler.emit_op(Operation::Return);

        let code = fn_compiler.bytecode;
        let arity = fn_decl.function.params.len() as u8;

        self.emit_op(Operation::Function(arity, name.clone(), code));

        // store the function in the current scope
        self.declare_variable(name.clone());
        if let Some((depth, index)) = self.resolve_local(&name) {
            if depth == 0 {
                self.emit_op(Operation::SetLocal(index));
            } else {
                // should not happen for declaration in current scope
                unreachable!();
            }
        } else {
            self.emit_op(Operation::StoreVar(name));
        }
    }

    fn compile_var_declator(&mut self, var_declator: &VarDeclarator) {
        match var_declator.init {
            Some(ref init) => self.compile_expr(init),
            None => self.emit_op(Operation::Undefined),
        }
        match &var_declator.name {
            Pat::Ident(name) => {
                let name_str = name.id.sym.to_string();
                self.declare_variable(name_str.clone());
                if let Some((depth, index)) = self.resolve_local(&name_str) {
                    if depth == 0 {
                        self.emit_op(Operation::SetLocal(index));
                    } else {
                        unreachable!("Declared variable should be local");
                    }
                } else {
                    // Should be unreachable if declare_variable works
                    self.emit_op(Operation::StoreVar(name_str));
                }
            }
            Pat::Array(_) => todo!(),
            Pat::Rest(_) => todo!(),
            Pat::Object(_) => todo!(),
            Pat::Assign(_) => todo!(),
            Pat::Invalid(_) => todo!(),
            Pat::Expr(_) => todo!(),
        }
    }

    fn compile_call(&mut self, expr: &CallExpr) {
        // push function onto stack
        match &expr.callee {
            Callee::Expr(callee_expr) => {
                self.compile_expr(callee_expr);
            }
            _ => unimplemented!("Super and Import calls not supported"),
        }

        // push args onto stack
        for arg in &expr.args {
            // TODO: figure out how to deal with span
            self.compile_expr(&arg.expr);
        }
        // push number of args onto stack
        let arg_len = expr.args.len().try_into().unwrap();
        self.emit_op(Operation::UInt64(arg_len));
        self.emit_op(Operation::Call);
    }

    fn compile_member_expr(&mut self, member_expr: &MemberExpr) {
        self.compile_expr(&member_expr.obj);

        if member_expr.prop.is_computed() {
            match member_expr.prop.as_computed() {
                Some(computed_prop_name) => {
                    self.compile_expr(&computed_prop_name.expr);
                }
                None => {}
            }
        } else if member_expr.prop.is_ident() {
            let name = member_expr.prop.as_ident().unwrap().sym.to_string();
            self.emit_op(Operation::LoadString(name));
        }

        self.emit_op(Operation::GetProperty);
    }

    fn compile_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit),
            Expr::Call(call_expr) => self.compile_call(call_expr),
            Expr::Member(member_expr) => self.compile_member_expr(member_expr),
            Expr::Ident(ident) => {
                let name = ident.sym.to_string();
                if let Some((depth, index)) = self.resolve_local(&name) {
                    if depth == 0 {
                        self.emit_op(Operation::GetLocal(index));
                    } else {
                        self.emit_op(Operation::GetUpvalue(depth, index));
                    }
                } else {
                    self.emit_op(Operation::LoadVar(name));
                }
            }
            Expr::Bin(bin_expr) => self.compile_bin_expr(bin_expr),
            Expr::Unary(unary_expr) => self.compile_unary_expr(unary_expr),
            Expr::Paren(paren_expr) => self.compile_expr(&paren_expr.expr),
            Expr::Assign(assign_expr) => self.compiler_assign_expr(assign_expr),
            Expr::Array(array_lit) => self.compile_array_lit(array_lit),
            Expr::Object(object_lit) => self.compile_object_lit(object_lit),
            _ => {
                println!("expression unimplemented:");
                println!("span={:?}", expr.type_id());
            }
        }
        // self.emit_op(Operation::Pop);
    }

    fn compiler_assign_expr(&mut self, expr: &AssignExpr) {
        match &expr.op {
            AssignOp::Assign => {
                // For member assignment, we need special handling
                match &expr.left {
                    AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                        // For obj.prop = value
                        // Need stack: [obj, propName, value]

                        // Compile the object
                        self.compile_expr(&member_expr.obj);

                        // Compile the property name
                        if member_expr.prop.is_computed() {
                            match member_expr.prop.as_computed() {
                                Some(computed_prop_name) => {
                                    self.compile_expr(&computed_prop_name.expr);
                                }
                                None => {}
                            }
                        } else if member_expr.prop.is_ident() {
                            let name = member_expr.prop.as_ident().unwrap().sym.to_string();
                            self.emit_op(Operation::LoadString(name));
                        }

                        // Compile the RHS value
                        self.compile_expr(&expr.right);

                        self.emit_op(Operation::SetProperty);
                    }
                    _ => {
                        // For other assignments, compile RHS first, then assign
                        self.compile_expr(&expr.right);
                        self.assign_to_target(&expr.left);
                    }
                }
            }
            _ => {
                // For compound assignments
                self.compile_compound_assign(expr);
            }
        }
    }

    fn assign_to_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Simple(simple_target) => match simple_target {
                SimpleAssignTarget::Ident(ident) => {
                    let name = ident.id.sym.to_string();
                    if let Some((depth, index)) = self.resolve_local(&name) {
                        if depth == 0 {
                            self.emit_op(Operation::SetLocal(index));
                        } else {
                            self.emit_op(Operation::SetUpvalue(depth, index));
                        }
                    } else {
                        self.emit_op(Operation::StoreVar(name));
                    }
                }
                _ => unimplemented!("assignment to this target not supported yet"),
            },
            AssignTarget::Pat(pat) => match pat {
                AssignTargetPat::Array(array_pat) => {
                    self.compile_array_destructuring(array_pat);
                }
                _ => unimplemented!("assignment to this pattern not supported yet"),
            },
        }
    }

    fn compile_compound_assign(&mut self, expr: &AssignExpr) {
        // Only supporting simple identifiers for now
        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                let name = ident.id.sym.to_string();

                // Load current value
                if let Some((depth, index)) = self.resolve_local(&name) {
                    if depth == 0 {
                        self.emit_op(Operation::GetLocal(index));
                    } else {
                        self.emit_op(Operation::GetUpvalue(depth, index));
                    }
                } else {
                    self.emit_op(Operation::LoadVar(name.clone()));
                }

                // Compile RHS
                self.compile_expr(&expr.right);

                // Perform operation
                match expr.op {
                    AssignOp::AddAssign => self.emit_op(Operation::Add),
                    AssignOp::SubAssign => self.emit_op(Operation::Sub),
                    AssignOp::MulAssign => self.emit_op(Operation::Mul),
                    AssignOp::DivAssign => self.emit_op(Operation::Div),
                    AssignOp::ModAssign => self.emit_op(Operation::Mod),
                    AssignOp::LShiftAssign => self.emit_op(Operation::LShift),
                    AssignOp::RShiftAssign => self.emit_op(Operation::RShift),
                    AssignOp::ZeroFillRShiftAssign => self.emit_op(Operation::ZeroFillRShift),
                    AssignOp::BitOrAssign => self.emit_op(Operation::BitOr),
                    AssignOp::BitXorAssign => self.emit_op(Operation::BitXor),
                    AssignOp::BitAndAssign => self.emit_op(Operation::BitAnd),
                    AssignOp::ExpAssign => self.emit_op(Operation::Exp),
                    AssignOp::AndAssign => self.emit_op(Operation::LogicalAnd),
                    AssignOp::OrAssign => self.emit_op(Operation::LogicalOr),
                    AssignOp::NullishAssign => self.emit_op(Operation::NullishCoalescing),
                    _ => unreachable!(),
                }

                // Store result
                if let Some((depth, index)) = self.resolve_local(&name) {
                    if depth == 0 {
                        self.emit_op(Operation::SetLocal(index));
                    } else {
                        self.emit_op(Operation::SetUpvalue(depth, index));
                    }
                } else {
                    self.emit_op(Operation::StoreVar(name));
                }
            }
            _ => unimplemented!("compound assignment to complex targets not supported yet"),
        }
    }

    fn compile_array_lit(&mut self, array_lit: &ArrayLit) {
        for elem in &array_lit.elems {
            match elem {
                Some(expr_or_spread) => {
                    self.compile_expr(&expr_or_spread.expr);
                }
                None => self.emit_op(Operation::Undefined),
            }
        }
        self.emit_op(Operation::Array(array_lit.elems.len() as u32));
    }

    fn compile_object_lit(&mut self, object_lit: &ObjectLit) {
        for prop in &object_lit.props {
            match prop {
                PropOrSpread::Prop(prop) => {
                    match &**prop {
                        Prop::KeyValue(key_value) => {
                            // Key
                            match &key_value.key {
                                PropName::Ident(ident) => {
                                    self.emit_op(Operation::LoadString(ident.sym.to_string()));
                                }
                                PropName::Str(s) => {
                                    self.emit_op(Operation::LoadString(
                                        s.value.to_string_lossy().into_owned(),
                                    ));
                                }
                                PropName::Num(n) => {
                                    self.emit_op(Operation::LoadString(n.value.to_string()));
                                }
                                PropName::Computed(computed) => {
                                    self.compile_expr(&computed.expr);
                                }
                                _ => unimplemented!("object key type not supported"),
                            }
                            // Value
                            self.compile_expr(&key_value.value);
                        }
                        _ => unimplemented!("object property type not supported"),
                    }
                }
                PropOrSpread::Spread(_) => unimplemented!("spread in object not supported"),
            }
        }
        self.emit_op(Operation::Object(object_lit.props.len() as u32));
    }

    fn compile_array_destructuring(&mut self, array_pat: &ArrayPat) {
        // The value to destructure is on top of the stack
        // We need to extract each element and assign it to the corresponding variable
        for (i, elem) in array_pat.elems.iter().enumerate() {
            if let Some(pat) = elem {
                // Duplicate the array on the stack (we'll need it for each element)
                if i < array_pat.elems.len() - 1 {
                    self.emit_op(Operation::Dup);
                }

                // Load the index
                self.emit_op(Operation::LoadFloat64(i as f64));

                // Get the property (array[i])
                self.emit_op(Operation::GetProperty);

                // Assign to the pattern
                match pat {
                    Pat::Ident(ident) => {
                        let name = ident.id.sym.to_string();
                        if let Some((depth, index)) = self.resolve_local(&name) {
                            if depth == 0 {
                                self.emit_op(Operation::SetLocal(index));
                            } else {
                                self.emit_op(Operation::SetUpvalue(depth, index));
                            }
                        } else {
                            self.emit_op(Operation::StoreVar(name));
                        }
                        // Pop the result of the assignment
                        if i < array_pat.elems.len() - 1 {
                            self.emit_op(Operation::Pop);
                        }
                    }
                    _ => unimplemented!("destructuring pattern not supported"),
                }
            }
        }
    }

    fn compile_bin_expr(&mut self, expr: &BinExpr) {
        self.compile_expr(&expr.left);
        self.compile_expr(&expr.right);
        match expr.op {
            BinaryOp::Add => self.emit_op(Operation::Add),
            BinaryOp::Sub => self.emit_op(Operation::Sub),
            BinaryOp::Mul => self.emit_op(Operation::Mul),
            BinaryOp::Div => self.emit_op(Operation::Div),
            BinaryOp::Mod => self.emit_op(Operation::Mod),
            BinaryOp::EqEq => self.emit_op(Operation::EqEq),
            BinaryOp::NotEq => self.emit_op(Operation::NotEq),
            BinaryOp::EqEqEq => self.emit_op(Operation::EqEqEq),
            BinaryOp::NotEqEq => self.emit_op(Operation::NotEqEq),
            BinaryOp::Lt => self.emit_op(Operation::Lt),
            BinaryOp::LtEq => self.emit_op(Operation::LtEq),
            BinaryOp::Gt => self.emit_op(Operation::Gt),
            BinaryOp::GtEq => self.emit_op(Operation::GtEq),
            BinaryOp::LShift => self.emit_op(Operation::LShift),
            BinaryOp::RShift => self.emit_op(Operation::RShift),
            BinaryOp::ZeroFillRShift => self.emit_op(Operation::ZeroFillRShift),
            BinaryOp::BitOr => self.emit_op(Operation::BitOr),
            BinaryOp::BitXor => self.emit_op(Operation::BitXor),
            BinaryOp::BitAnd => self.emit_op(Operation::BitAnd),
            BinaryOp::In => self.emit_op(Operation::In),
            BinaryOp::InstanceOf => self.emit_op(Operation::InstanceOf),
            BinaryOp::LogicalAnd => self.emit_op(Operation::LogicalAnd),
            BinaryOp::LogicalOr => self.emit_op(Operation::LogicalOr),
            BinaryOp::Exp => self.emit_op(Operation::Exp),
            BinaryOp::NullishCoalescing => self.emit_op(Operation::NullishCoalescing),
        }
    }

    fn compile_unary_expr(&mut self, expr: &UnaryExpr) {
        self.compile_expr(&expr.arg);
        match expr.op {
            UnaryOp::Minus => self.emit_op(Operation::UnaryMinus),
            UnaryOp::Plus => self.emit_op(Operation::UnaryPlus),
            UnaryOp::Bang => self.emit_op(Operation::LogicalNot),
            UnaryOp::Tilde => self.emit_op(Operation::BitwiseNot),
            UnaryOp::TypeOf => self.emit_op(Operation::TypeOf),
            UnaryOp::Void => self.emit_op(Operation::Void),
            UnaryOp::Delete => self.emit_op(Operation::Delete),
        }
    }

    fn compile_lit(&mut self, lit: &Lit) {
        match lit {
            Lit::Str(s) => self.emit_op(Operation::LoadString(
                s.value.to_string_lossy().into_owned(),
            )),
            Lit::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            Lit::Bool(b) => self.emit_op(Operation::Bool(b.value)),
            Lit::Null(_) => self.emit_op(Operation::Null),
            Lit::Regex(r) => {
                self.emit_op(Operation::Regex(
                    r.exp.to_string().clone(),
                    r.flags.to_string().clone(),
                ));
            }
            Lit::BigInt(_) => todo!(),
            Lit::JSXText(_) => unimplemented!(),
        }
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
            _ => {}
        }
    }

    fn emit_string(&mut self, s: &str) {
        self.bytecode.extend_from_slice(s.as_bytes());
        self.bytecode.push(0);
    }
}

impl Visit for Compiler<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        self.compile_expr(expr);
    }
    fn visit_var_decl(&mut self, n: &VarDecl) {
        self.compile_var_decl(n);
    }
}
