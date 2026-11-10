use kumojs::Compiler;
use std::fs::File;
use std::io::Write;
use std::process::Command;
use swc_common::sync::Lrc;
use swc_common::SourceMap;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};

fn compile_source(source: &str) -> Vec<u8> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(swc_common::FileName::Anon.into(), source.to_string());

    let lexer = Lexer::new(
        Syntax::Es(Default::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    if let Some(e) = parser.take_errors().into_iter().next() {
        panic!("Parser error: {:?}", e);
    }

    let module = parser.parse_module().expect("failed to parse module");

    let mut compiler = Compiler::new();
    compiler.compile(&module)
}

fn run_test(source: &str, expected_result: &str) {
    let bytecode = compile_source(source);

    // bytecode to temp file
    let temp_dir = std::env::temp_dir();
    let bytecode_path = temp_dir.join(format!(
        "test_{}_{:?}.kumo",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::thread::current().id()
    ));
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

    assert_eq!(result, expected_result);
}

#[test]
fn test_function_declaration_and_call() {
    run_test(
        r#"
        function x(a, b) {
            return a * b;
        }
        const a = 2;
        const b = 5;
        x(a, b)
        "#,
        "10",
    );
}

#[test]
fn test_function_scope_access() {
    run_test(
        r#"
        const globalVal = 10;
        function getGlobal() {
            return globalVal;
        }
        getGlobal()
        "#,
        "10",
    );
}

#[test]
fn test_execution_continues_after_call() {
    run_test(
        r#"
        function add(a, b) {
            return a + b;
        }
        let result = add(2, 3);
        result + 1
        "#,
        "6",
    );
}

#[test]
fn test_console_log_function_result() {
    run_test(
        r#"
        function add(a, b) {
            return a + b;
        }
        console.log(add(2, 3))
        "#,
        "5\nundefined",
    );
}

#[test]
fn test_escaping_function() {
    // This test relies on test_runner.js checking for 'vm.global.c' and calling it with (2, 5)
    // The output will include "External call result: 10"

    let source = r#"
        function x(a, b) {
            return a * b;
        }
        window.c = x;
    "#;

    let bytecode = compile_source(source);
    let temp_dir = std::env::temp_dir();
    let bytecode_path = temp_dir.join(format!("test_escaping_{}.kumo", std::process::id()));
    let mut file = File::create(&bytecode_path).expect("failed to create temp file");
    file.write_all(&bytecode).expect("failed to write bytecode");

    let verification_script = r#"
        if (vm.global && typeof vm.global.c === 'function') {
            const result = vm.global.c(2, 5);
            console.log("External call result:", result);
        }
    "#;

    let output = Command::new("node")
        .arg("src/vm/test_runner.js")
        .arg(&bytecode_path)
        .arg(verification_script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to execute node process");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("Node.js process failed:\n{}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("STDOUT:\n{}", stdout);
    assert!(stdout.contains("External call result: 10"));
}
