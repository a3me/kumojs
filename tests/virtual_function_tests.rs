mod common;

use common::{run_test, run_vm};

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
