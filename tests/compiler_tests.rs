use kumojs::Compiler;
use std::fs::File;
use std::io::Write;
use std::process::Command;
use swc_common::sync::Lrc;
use swc_common::SourceMap;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};

fn compile_source(source: &str) -> Vec<u8> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(swc_common::FileName::Anon, source.into());

    let lexer = Lexer::new(
        Syntax::Es(Default::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    for e in parser.take_errors() {
        panic!("Parser error: {:?}", e);
    }

    let module = parser.parse_module().expect("failed to parse module");

    let mut compiler = Compiler::new();
    compiler.compile(&module)
}

#[test]
fn test_one_plus_one() {
    // compile source
    let source = "1 + 1";
    let bytecode = compile_source(source);

    // bytecode to temp file
    let temp_dir = std::env::temp_dir();
    let bytecode_path = temp_dir.join("test_one_plus_one.kumo");
    let mut file = File::create(&bytecode_path).expect("failed to create temp file");
    file.write_all(&bytecode).expect("failed to write bytecode");

    // run vm with node
    let output = Command::new("node")
        .arg("src/vm/test_runner.js")
        .arg(&bytecode_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to execute node process");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("Node.js process failed:\n{}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.trim();

    // assert result
    assert_eq!(result, "2");
}
