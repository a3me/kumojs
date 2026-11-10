mod common;

use common::{compile, run_test, run_vm};

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
        window.console.log(add(2, 3))
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

    let verification_script = r#"
        if (vm.global && typeof vm.global.c === 'function') {
            const result = vm.global.c(2, 5);
            console.log("External call result:", result);
        }
    "#;

    let stdout = run_vm(source, Some(verification_script));
    println!("STDOUT:\n{}", stdout);
    assert!(stdout.contains("External call result: 10"));
}

#[test]
fn test_demo_example_script_runs() {
    // the dev server compiles this file for the demo page
    let source = std::fs::read_to_string("src/scripts/example.js").unwrap();
    let stdout = run_vm(&source, Some("console.log('c:', vm.global.c(3, 4))"));
    assert!(stdout.starts_with("10\n"), "{stdout}");
    assert!(stdout.contains("c: 12"), "{stdout}");
}

/// Directly-called functions compile to the internal opcode (no host wrapper); any other
/// use keeps the host-callable one. Hoisted declarations come first in the bytecode.
#[test]
fn test_internal_vs_escaping_function_opcode() {
    const HOST_CALLABLE: u8 = 0x37;
    const INTERNAL: u8 = 0x4b;
    let first_op = |src: &str| compile(src)[0];

    assert_eq!(first_op("function f() {} f();"), INTERNAL);
    assert_eq!(first_op("function f(n) { return n ? f(n - 1) : 0; } f(3);"), INTERNAL);
    assert_eq!(first_op("function f() {} window.x = f;"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} [1].map(f);"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} let a = [f];"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} typeof f;"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} new f();"), HOST_CALLABLE);
    assert_eq!(first_op("function f() { return f; } f();"), HOST_CALLABLE);
}
