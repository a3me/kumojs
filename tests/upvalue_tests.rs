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
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result = stdout.trim();

    if result != expected_result {
        println!("STDOUT:\n{}", stdout);
        println!("STDERR:\n{}", stderr);
    }

    assert_eq!(result, expected_result);
}

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
            return Math.PI;
        }
        f()
        ",
        "3.141592653589793",
    );
}

#[test]
fn test_global_set_from_function() {
    // Assuming 'global_var' is not defined, it should be treated as global
    // But in our compiler, if it's not resolved as local/upvalue, it becomes LoadVar/StoreVar
    // which accesses this.global or this[varName]
    run_test(
        "
        function f() {
            global_var = 100;
        }
        f();
        global_var
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
                console.log(123);
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
