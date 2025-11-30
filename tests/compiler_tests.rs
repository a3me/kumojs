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
fn test_one_plus_one() {
    run_test("1 + 1", "2");
}

#[test]
fn test_subtraction() {
    run_test("1 - 1", "0");
    run_test("10 - 5", "5");
}

#[test]
fn test_multiplication() {
    run_test("2 * 3", "6");
    run_test("5 * 5", "25");
}

#[test]
fn test_division() {
    run_test("6 / 2", "3");
    run_test("10 / 2", "5");
}

#[test]
fn test_modulo() {
    run_test("5 % 2", "1");
    run_test("10 % 3", "1");
}

#[test]
fn test_complex_expression() {
    run_test("1 + 2 * 3", "7");
    run_test("(1 + 2) * 3", "9");
}

#[test]
fn test_lshift() {
    run_test("2 << 1", "4");
    run_test("5 << 2", "20");
}

#[test]
fn test_rshift() {
    run_test("8 >> 1", "4");
    run_test("20 >> 2", "5");
}

#[test]
fn test_zero_fill_rshift() {
    run_test("8 >>> 1", "4");
    run_test("16 >>> 2", "4");
}

#[test]
fn test_bit_or() {
    run_test("5 | 3", "7");
    run_test("12 | 10", "14");
}

#[test]
fn test_bit_xor() {
    run_test("5 ^ 3", "6");
    run_test("12 ^ 10", "6");
}

#[test]
fn test_bit_and() {
    run_test("5 & 3", "1");
    run_test("12 & 10", "8");
}

// Logical operations
#[test]
fn test_logical_or() {
    run_test("true || false", "true");
    run_test("false || false", "false");
    run_test("0 || 5", "5");
}

#[test]
fn test_logical_and() {
    run_test("true && false", "false");
    run_test("true && true", "true");
    run_test("5 && 10", "10");
}

// Comparison operations
#[test]
fn test_equality() {
    run_test("5 == 5", "true");
    run_test("5 != 3", "true");
    run_test("5 != 5", "false");
}

#[test]
fn test_strict_equality() {
    run_test("5 === 5", "true");
    run_test("5 === \"5\"", "false");
    run_test("5 !== \"5\"", "true");
}

#[test]
fn test_less_than() {
    run_test("3 < 5", "true");
    run_test("5 < 3", "false");
    run_test("3 <= 3", "true");
}

#[test]
fn test_greater_than() {
    run_test("5 > 3", "true");
    run_test("3 > 5", "false");
    run_test("5 >= 5", "true");
}

// Other operations
#[test]
fn test_exponentiation() {
    run_test("2 ** 3", "8");
    run_test("5 ** 2", "25");
}

#[test]
fn test_nullish_coalescing() {
    run_test("null ?? 5", "5");
    run_test("undefined ?? 10", "10");
    run_test("0 ?? 5", "0");
}

// Local variable tests
#[test]
fn test_local_variable_declaration() {
    run_test("let x = 5; x", "5");
}

#[test]
fn test_local_variable_arithmetic() {
    run_test("let x = 10; let y = 5; x + y", "15");
}

#[test]
fn test_multiple_locals() {
    run_test("let a = 1; let b = 2; let c = 3; a + b + c", "6");
}

#[test]
fn test_expression_statement_pop() {
    // This test verifies that intermediate expression statements are popped
    // Without proper pop, the stack would grow and local offsets would be wrong
    run_test("1 + 1; let x = 5; x", "5");
    run_test("10; 20; let y = 3; y", "3");
}

#[test]
fn test_console_log() {
    // console.log(5) prints "5" to stdout
    // The expression returns undefined
    // test_runner.js prints the return value (undefined)
    // So we expect "5\nundefined"
    run_test("let x = 5; console.log(x)", "5\nundefined");
}

#[test]
fn test_property_access() {
    // Math.PI is a property on the global Math object
    // We can't easily test console properties as they are functions
    // But we can test Math.PI if it exists in Node global
    // Node has global.Math
    run_test("Math.PI", "3.141592653589793");
}

#[test]
fn test_global_shadowing() {
    // Shadow global console with local variable
    run_test("let console = 10; console", "10");
}

#[test]
fn test_nested_property_access() {
    // Test accessing a property of a property and calling it
    // Math.max(1, 2)
    run_test("Math.max(1, 2)", "2");
}

#[test]
fn test_unary_operations() {
    run_test("-5", "-5");
    run_test("+5", "5");
    run_test("!true", "false");
    run_test("!false", "true");
    run_test("~5", "-6");
    run_test("typeof 5", "number");
    run_test("typeof 'hello'", "string");
    run_test("void 0", "undefined");
}