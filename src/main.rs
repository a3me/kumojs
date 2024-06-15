use serde::Serialize;
use serde_json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use swc_common::errors::{ColorConfig, Handler};
use swc_common::sync::Lrc;
use swc_common::SourceMap;
use swc_ecma_ast::{Expr, Ident, Lit, MemberExpr, MemberProp, Module};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitMut, VisitMutWith, VisitWith};

struct Compiler {
    bytecode: Vec<u8>,
    disassembly: Vec<DisassembledOp>,
    scopes: Vec<HashMap<String, usize>>,
    current_scope_depth: usize,
}

#[derive(Debug)]
struct DisassembledOp {
    op: &'static str,
    opcode: u8,
    offset: usize,
}

#[derive(Debug)]
enum Operation {
    LoadString(String),
    LoadInt32(i32),
    StoreVar(String),
    LoadVar(String),
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            bytecode: Vec::new(),
            scopes: Vec::new(),
            disassembly: Vec::new(),
            current_scope_depth: 0,
        }
    }

    fn compile(&mut self, module: &Module) -> Vec<u8> {
        module.visit_with(self);
        self.bytecode.clone()
    }

    fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
        self.scopes.pop();
    }

    fn declare_variable(&mut self, name: &str) {
        let scope = self.scopes.last_mut().unwrap();
        scope.insert(name.to_string(), self.current_scope_depth);
    }

    fn resolve_variable(&self, name: &str) -> Option<usize> {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if let Some(depth) = scope.get(name) {
                return Some(self.current_scope_depth - i);
            }
        }
        None
    }

    fn compile_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit),
            _ => unimplemented!(),
        }
    }

    fn compile_lit(&mut self, lit: &Lit) {
        match lit {
            Lit::Str(s) => {
                self.emit_op(Operation::LoadString(s.value.to_string().clone()));
            }
            Lit::Num(n) => {
                self.bytecode.push(0x02);
                let bytes = n.value.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            _ => unimplemented!(),
        }
    }

    fn emit_op(&mut self, op: Operation) {
        match op {
            Operation::LoadString(s) => {
                self.disassembly.push(DisassembledOp {
                    op: "OP_LOAD_STRING",
                    opcode: 0x01,
                    offset: self.bytecode.len(),
                });
                self.bytecode.push(0x01);
                self.bytecode.extend_from_slice(s.as_bytes());
                self.bytecode.push(0x00);
            }
            Operation::LoadInt32(n) => {
                self.bytecode.push(0x02);
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            _ => unimplemented!(),
        }
    }
}

impl Visit for Compiler {
    fn visit_expr(&mut self, expr: &Expr) {
        self.compile_expr(expr);
        expr.visit_children_with(self);
    }
}

fn main() {
    let cm: Lrc<SourceMap> = Default::default();

    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    let fm = cm
        .load_file(Path::new("scripts/example.js"))
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

    let module = parser
        .parse_module()
        .map_err(|e| {
            // Unrecoverable fatal error occurred
            e.into_diagnostic(&handler).emit()
        })
        .expect("failed to parse module");

    let mut compiler = Compiler::new();

    compiler.compile(&module);

    let bytecode_json =
        serde_json::to_string(&compiler.bytecode).expect("failed to serialize bytecode");

    let path = Path::new("vm/bytecode.json");

    let mut file = File::create(path).expect("failed to create bytecode file");
    
    file.write_all(bytecode_json.as_bytes())
        .expect("failed to write to bytecode file");

    println!("{:?}", compiler.disassembly);
}
