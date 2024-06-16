use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::path::Path;
use std::time::Instant;
use swc_common::errors::{ColorConfig, Handler};
use swc_common::sync::Lrc;
use swc_common::SourceMap;
use swc_ecma_ast::{Expr, Ident, Lit, MemberExpr, MemberProp, Module, Pat, VarDecl, VarDeclarator};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

pub struct Compiler<'a> {
    bytecode: Vec<u8>,
    current_scope_depth: usize,
    scope: HashMap<String, usize>,
    enclosing: Option<&'a Compiler<'a>>,
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

#[derive(Debug)]
enum Operation {
    Return,
    LoadString(String),
    LoadFloat64(f64),
    Bool(bool),
    Pop,
    Null,
    Undefined,
    Regex(String, String),
    StoreVar(String),
    LoadVar(String),
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
        }
    }
}

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Compiler {
            bytecode: Vec::new(),
            scope: HashMap::new(),
            current_scope_depth: 0,
            enclosing: None,
        }
    }

    fn new_enclosing(&'a self) -> Compiler<'a> {
        Compiler {
            bytecode: Vec::new(),
            scope: HashMap::new(),
            current_scope_depth: self.current_scope_depth + 1,
            enclosing: None,
        }
    }

    pub fn compile_file(&mut self, path: &Path) -> Vec<u8> {
        let cm: Lrc<SourceMap> = Default::default();

        let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

        let fm = cm
            .load_file(path)
            .expect("failed to load scripts/example.js");

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

        let module = parser
            .parse_module()
            .map_err(|e| {
                // Unrecoverable fatal error occurred
                e.into_diagnostic(&handler).emit()
            })
            .expect("failed to parse module");

        println!("parsing took {:?}", Instant::now() - parse_start);

        self.compile(&module)
    }

    pub fn compile(&mut self, module: &Module) -> Vec<u8> {
        let compile_start = Instant::now();
        module.visit_with(self);
        println!("compiling took {:?}", Instant::now() - compile_start);
        self.bytecode.clone()
    }

    // fn enter_scope(&mut self) {
    //     self.current_scope_depth += 1;
    //     self.scopes.push(HashMap::new());
    // }

    // fn exit_scope(&mut self) {
    //     self.current_scope_depth -= 1;
    //     self.scopes.pop();
    // }

    fn declare_variable(&mut self, name: String) {
        self.scope.insert(name, self.current_scope_depth);
    }

    // fn resolve_variable(&self, name: &str) -> Option<usize> {
    //     for (i, scope) in self.scopes.iter().enumerate().rev() {
    //         if let Some(depth) = scope.get(name) {
    //             return Some(self.current_scope_depth - i);
    //         }
    //     }
    //     None
    // }

    // compile variable declarations
    fn compile_var_decl(&mut self, var_decl: &VarDecl) {
        for decl in var_decl.decls.iter() {
            self.compile_var_declator(decl);
        }
    }

    fn compile_var_declator(&mut self, var_declator: &VarDeclarator) {
        match var_declator.init {
            Some(ref init) => self.compile_expr(init),
            None => self.emit_op(Operation::Undefined),
        }
        match &var_declator.name {
            Pat::Ident(name) => {
                println!("{:?}", name.id.sym.to_string());
                self.declare_variable(name.id.sym.to_string());
                self.emit_op(Operation::StoreVar(name.id.to_string()));
            }
            Pat::Array(_) => todo!(),
            Pat::Rest(_) => todo!(),
            Pat::Object(_) => todo!(),
            Pat::Assign(_) => todo!(),
            Pat::Invalid(_) => todo!(),
            Pat::Expr(_) => todo!(),
        }
    }

    fn compile_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit),
            _ => unimplemented!(),
        }
        self.emit_op(Operation::Pop);
    }

    fn compile_lit(&mut self, lit: &Lit) {
        match lit {
            Lit::Str(s) => {
                self.emit_op(Operation::LoadString(s.value.to_string().clone()));
            }
            Lit::Num(n) => {
                self.emit_op(Operation::LoadFloat64(n.value));
            }
            Lit::Bool(b) => {
                self.emit_op(Operation::Bool(b.value));
            }
            Lit::Null(_) => {
                self.emit_op(Operation::Null);
            }
            Lit::Regex(r) => {
                self.emit_op(Operation::Regex(
                    r.exp.to_string().clone(),
                    r.flags.to_string().clone(),
                ));
            }
            Lit::BigInt(_) => todo!(),
            Lit::JSXText(_) => todo!(),
        }
    }

    fn emit_op(&mut self, op: Operation) {
        self.bytecode.push(op.get_opcode());
        match op {
            Operation::LoadString(s) => {
                self.emit_string(&s);
            }
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
            Operation::StoreVar(name) => {
                self.emit_string(&name);
            }
            Operation::Regex(exp, flags) => {
                self.emit_string(&exp);
                self.emit_string(&flags);
            }
            Operation::Return => {}
            Operation::Undefined => {}
            Operation::Pop => {}
            Operation::Null => {}
            Operation::LoadVar(_) => todo!(),
        }
    }

    fn emit_string(&mut self, s: &str) {
        self.bytecode.extend_from_slice(s.as_bytes());
        self.bytecode.push(0x00);
    }
}

impl Visit for Compiler<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        self.compile_expr(expr);
        expr.visit_children_with(self);
    }
    fn visit_var_decl(&mut self, n: &VarDecl) {
        self.compile_var_decl(n);
        n.visit_children_with(self);
    }
}
