mod common;

use common::run_test;

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
    run_test("let x = 5; window.console.log(x)", "5\nundefined");
}

#[test]
fn test_property_access() {
    // Math.PI is a property on the global Math object
    // We can't easily test console properties as they are functions
    // But we can test Math.PI if it exists in Node global
    // Node has global.Math
    run_test("window.Math.PI", "3.141592653589793");
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
    run_test("window.Math.max(1, 2)", "2");
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

#[test]
fn test_assignment() {
    run_test("let x = 1; x = 2; x", "2");
    run_test("let x = 1; let y = (x = 2); y", "2");
}

#[test]
fn test_array_literal() {
    run_test("let arr = [1, 2, 3]; arr[0]", "1");
    run_test("let arr = [1, 2, 3]; arr[1]", "2");
    run_test("let arr = [1, 2, 3]; arr[2]", "3");
}

#[test]
fn test_object_literal() {
    run_test("let obj = {a: 1, b: 2}; obj.a", "1");
    run_test("let obj = {a: 1, b: 2}; obj.b", "2");
}

#[test]
fn test_array_destructuring() {
    run_test("let x, y; [x, y] = [1, 2]; x", "1");
    run_test("let x, y; [x, y] = [1, 2]; y", "2");
}

#[test]
fn test_compound_assignment() {
    run_test("let x = 5; x += 3; x", "8");
    run_test("let x = 10; x -= 3; x", "7");
    run_test("let x = 4; x *= 2; x", "8");
    run_test("let x = 10; x /= 2; x", "5");
    run_test("let x = 10; x %= 3; x", "1");
}

#[test]
fn test_property_assignment() {
    run_test("let obj = {a: 1}; obj.a = 5; obj.a", "5");
    run_test("let obj = {a: 1}; obj.b = 10; obj.b", "10");
    run_test("let arr = [1, 2, 3]; arr[0] = 10; arr[0]", "10");
}

#[test]
fn test_unsupported_syntax_is_an_error_with_location() {
    let err = kumojs::Compiler::new()
        .compile_source("input.js", "let x = [1];\nfor (let y of x) {}")
        .unwrap_err();
    assert_eq!(err.to_string(), "unsupported for-of loop at input.js:2:1");

    let err = kumojs::Compiler::new()
        .compile_source("input.js", "function f() {\n  return () => 1;\n}")
        .unwrap_err();
    assert_eq!(err.to_string(), "unsupported arrow function at input.js:2:10");
}

#[test]
fn test_parse_error_has_location() {
    let err = kumojs::Compiler::new()
        .compile_source("input.js", "let = ;")
        .unwrap_err();
    assert!(err.to_string().contains("input.js:1:"), "{err}");
}

#[test]
fn test_undeclared_variables_are_compile_errors() {
    let compile = |src| kumojs::Compiler::new().compile_source("input.js", src);

    let err = compile("Math.max(1, 2)").unwrap_err();
    assert_eq!(err.to_string(), "unsupported undeclared variable `Math` at input.js:1:1");

    let err = compile("function f() {\n  x = 1;\n}").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported assignment to undeclared variable `x` at input.js:2:3"
    );

    let err = compile("window = 1").unwrap_err();
    assert_eq!(err.to_string(), "unsupported assignment to `window` at input.js:1:1");

    // block-scoped names are gone after the block
    let err = compile("{ let inner = 1; }\ninner").unwrap_err();
    assert_eq!(err.to_string(), "unsupported undeclared variable `inner` at input.js:2:1");

    assert!(compile("typeof window; typeof globalThis; window.Math.max(1, 2)").is_ok());
}
