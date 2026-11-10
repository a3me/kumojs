mod common;

use common::run_test;

#[test]
fn test_upvalue_read() {
    run_test(
        "
        let x = 10;
        function f() {
            return x;
        }
        f()
        ",
        "10",
    );
}

#[test]
fn test_upvalue_set() {
    run_test(
        "
        let x = 10;
        function f() {
            x = 20;
        }
        f();
        x
        ",
        "20",
    );
}

#[test]
fn test_upvalue_compound_set() {
    run_test(
        "
        let x = 10;
        function f() {
            x += 5;
        }
        f();
        x
        ",
        "15",
    );
}

#[test]
fn test_deep_upvalue_read() {
    run_test(
        "
        let x = 10;
        function f() {
            function g() {
                return x;
            }
            return g();
        }
        f()
        ",
        "10",
    );
}

#[test]
fn test_deep_upvalue_set() {
    run_test(
        "
        let x = 10;
        function f() {
            function g() {
                x = 30;
            }
            g();
        }
        f();
        x
        ",
        "30",
    );
}

#[test]
fn test_shadowing_local() {
    run_test(
        "
        let x = 10;
        function f() {
            let x = 20;
            return x;
        }
        f()
        ",
        "20",
    );
}

#[test]
fn test_shadowing_outer() {
    run_test(
        "
        let x = 10;
        function f() {
            let x = 20;
            return x;
        }
        f();
        x
        ",
        "10",
    );
}

#[test]
fn test_global_access() {
    run_test(
        "
        function f() {
            return window.Math.PI;
        }
        f()
        ",
        "3.141592653589793",
    );
}

#[test]
fn test_global_set_from_function() {
    // globals are only reachable through `window`
    run_test(
        "
        function f() {
            window.global_var = 100;
        }
        f();
        window.global_var
        ",
        "100",
    );
}

#[test]
fn test_closure_state_persistence() {
    // This tests if the closure keeps the upvalue alive while the parent is running.
    // Since we don't support escaping closures yet (stack truncation),
    // we only test nested calls.
    run_test(
        "
        let counter = 0;
        function inc() {
            counter += 1;
            return counter;
        }
        inc();
        inc()
        ",
        "2",
    );
}

#[test]
fn test_console_log_from_nested() {
    run_test(
        "
        function f() {
            function g() {
                window.console.log(123);
            }
            g();
        }
        f()
        ",
        "123\nundefined",
    );
}

#[test]
fn test_escaping_closure() {
    run_test(
        "
        function makeCounter() {
            let count = 0;
            function increment() {
                count += 1;
                return count;
            }
            return increment;
        }
        let counter = makeCounter();
        counter();
        counter()
        ",
        "2",
    );
}
