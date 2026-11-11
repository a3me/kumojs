//! Compiler tests that inspect the compiler directly (error messages, emitted opcodes),
//! with no VM run. Behavioural tests live in `tests/fixtures`, compared against Node.

use kumojs::Compiler;

fn compile(source: &str) -> Vec<u8> {
    Compiler::new()
        .compile_source("input.js", source)
        .unwrap_or_else(|e| panic!("compile failed: {e}"))
}

/// Directly-called functions compile to the internal opcode (no host wrapper); any other
/// use keeps the host-callable one. Hoisted declarations come first in the bytecode.
#[test]
fn internal_vs_escaping_function_opcode() {
    const HOST_CALLABLE: u8 = 0x37;
    const INTERNAL: u8 = 0x4b;
    const NEW_BOX: u8 = 0x4e;
    // skip boxes made for captured names (a recursive function captures its own name)
    let first_op = |src: &str| {
        let code = compile(src);
        let mut i = 0;
        while code[i] == NEW_BOX {
            i += 2;
        }
        code[i]
    };

    assert_eq!(first_op("function f() {} f();"), INTERNAL);
    assert_eq!(
        first_op("function f(n) { return n ? f(n - 1) : 0; } f(3);"),
        INTERNAL
    );
    assert_eq!(first_op("function f() {} window.x = f;"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} [1].map(f);"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} let a = [f];"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} typeof f;"), HOST_CALLABLE);
    assert_eq!(first_op("function f() {} new f();"), HOST_CALLABLE);
    assert_eq!(first_op("function f() { return f; } f();"), HOST_CALLABLE);
}

#[test]
fn unsupported_syntax_is_an_error_with_location() {
    let err = Compiler::new()
        .compile_source("input.js", "let x = [1];\nexport { x };")
        .unwrap_err();
    assert_eq!(err.to_string(), "unsupported import/export at input.js:2:1");

    let err = Compiler::new()
        .compile_source("input.js", "function f() {\n  return import(\"x\");\n}")
        .unwrap_err();
    assert_eq!(err.to_string(), "unsupported dynamic import at input.js:2:10");
}

#[test]
fn parse_error_has_location() {
    let err = Compiler::new()
        .compile_source("input.js", "let = ;")
        .unwrap_err();
    assert!(err.to_string().contains("input.js:1:"), "{err}");
}

#[test]
fn undeclared_variables_are_compile_errors() {
    let compile = |src| Compiler::new().compile_source("input.js", src);

    let err = compile("Math.max(1, 2)").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported undeclared variable `Math` at input.js:1:1"
    );

    let err = compile("function f() {\n  x = 1;\n}").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported assignment to undeclared variable `x` at input.js:2:3"
    );

    let err = compile("window = 1").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported assignment to `window` at input.js:1:1"
    );

    // block-scoped names are gone after the block
    let err = compile("{ let inner = 1; }\ninner").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported undeclared variable `inner` at input.js:2:1"
    );

    assert!(compile("typeof window; typeof globalThis; window.Math.max(1, 2)").is_ok());
}

#[test]
fn private_name_errors() {
    let compile = |src| Compiler::new().compile_source("input.js", src);

    let err = compile("class A {\n  m() { return this.#nope; }\n}").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported undeclared private name `#nope` at input.js:2:21"
    );

    // private names are only visible inside their class body
    let err = compile("class A { #x = 1; }\nclass B { m(o) { return #x in o; } }").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported undeclared private name `#x` at input.js:2:25"
    );

    let err = compile("class A { #x = 1; #x() {} }").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported duplicate private name `#x` at input.js:1:19"
    );

    // a getter and setter share a name
    assert!(compile("class A { get #x() { return 1; } set #x(v) {} }").is_ok());
}
