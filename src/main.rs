use serde_json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
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
    LoadFloat64(f64),
    Bool(bool),
    Pop,
    Null,
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
            _ => unimplemented!(),
        }
    }

    fn get_name(&self) -> &'static str {
        match self {
            Operation::LoadString(_) => "OP_LOAD_STRING",
            Operation::LoadFloat64(_) => "OP_LOAD_FLOAT64",
            Operation::Bool(_) => "OP_LOAD_BOOL",
            Operation::Pop => "OP_POP",
            Operation::Null => "OP_NULL",
            Operation::Regex(_, _) => "OP_REGEX",
            Operation::StoreVar(_) => "OP_STORE_VAR",
            Operation::LoadVar(_) => "OP_LOAD_VAR",
        }
    }

    fn disassemble(&self, offset: usize) -> DisassembledOp {
        DisassembledOp {
            op: self.get_name(),
            opcode: self.get_opcode(),
            offset: offset,
        }
    }
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

    // fn enter_scope(&mut self) {
    //     self.current_scope_depth += 1;
    //     self.scopes.push(HashMap::new());
    // }

    // fn exit_scope(&mut self) {
    //     self.current_scope_depth -= 1;
    //     self.scopes.pop();
    // }

    // fn declare_variable(&mut self, name: &str) {
    //     let scope = self.scopes.last_mut().unwrap();
    //     scope.insert(name.to_string(), self.current_scope_depth);
    // }

    // fn resolve_variable(&self, name: &str) -> Option<usize> {
    //     for (i, scope) in self.scopes.iter().enumerate().rev() {
    //         if let Some(depth) = scope.get(name) {
    //             return Some(self.current_scope_depth - i);
    //         }
    //     }
    //     None
    // }

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
            _ => unimplemented!(),
        }
    }

    fn emit_string(&mut self, s: &str) {
        self.bytecode.extend_from_slice(s.as_bytes());
        self.bytecode.push(0x00);
    }

    fn emit_op(&mut self, op: Operation) {
        // append disassembly
        self.disassembly.push(op.disassemble(self.bytecode.len()));

        // push opcode
        self.bytecode.push(op.get_opcode());

        match op { // match opcode and push operands
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
            Operation::Regex(exp, flags) => {
                self.emit_string(&exp);
                self.emit_string(&flags);
            }
            Operation::Pop => {}
            Operation::Null => {}
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

    // compile input js
    let compile_start = Instant::now();

    let mut compiler = Compiler::new();

    compiler.compile(&module);

    println!("compiling took {:?}", Instant::now() - compile_start);

    let bytecode_json =
        serde_json::to_string(&compiler.bytecode).expect("failed to serialize bytecode");

    let path = Path::new("vm/bytecode.json");

    let mut file = File::create(path).expect("failed to create bytecode file");

    file.write_all(bytecode_json.as_bytes())
        .expect("failed to write to bytecode file");

    println!("{:?}", compiler.disassembly);
}
