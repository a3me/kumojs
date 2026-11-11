//! Opcode shuffling is behaviour-preserving: bytecode compiled under a seed disassembles to
//! the same listing as the canonical build, and the embedded VM for that seed runs a program
//! to the same output as the canonical one.

use kumojs::disasm::{disassemble, disassemble_with};
use kumojs::embed::compile_embedded_source;
use kumojs::opcode::OpcodeMap;
use kumojs::Compiler;
use std::process::Command;

const PROGRAMS: &[&str] = &[
    "window.console.log(1 + 2 * 3, [3, 1, 2].sort().join(','));",
    "function fib(n){ return n < 2 ? n : fib(n-1)+fib(n-2); } window.console.log(fib(12));",
    "class A { constructor(v){ this.v = v; } get d(){ return this.v*2; } } window.console.log(new A(21).d);",
    "let t = 0; for (let x of [1,2,3,4]) t += x*x; window.console.log(t);",
];

#[test]
fn seeded_bytecode_disassembles_canonically() {
    for src in PROGRAMS {
        let canonical = Compiler::new().compile_source("p.js", src).unwrap();
        let listing = disassemble(&canonical).unwrap();
        for seed in [0u64, 1, 42, 7777] {
            let map = OpcodeMap::from_seed(seed);
            let shuffled = Compiler::new()
                .with_opcode_map(map.clone())
                .compile_source("p.js", src)
                .unwrap();
            assert_eq!(
                listing,
                disassemble_with(&shuffled, &map).unwrap(),
                "seed {seed} listing differs for {src:?}"
            );
        }
    }
}

/// Run `js` in Node and return its stdout (scripts are written against `window`).
fn node_run(js: &str) -> String {
    let path = std::env::temp_dir().join(format!("kumo_shuffle_{}.js", unique()));
    std::fs::write(&path, format!("globalThis.window = globalThis;\n{js}")).unwrap();
    let out = Command::new("node").arg(&path).output().expect("run node");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "node failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn unique() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    std::process::id() as usize * 10_000 + N.fetch_add(1, Ordering::Relaxed)
}

/// The standalone VM+program for a shuffled build runs to the same output as the program
/// does natively in Node — so renumbering opcodes doesn't change behaviour end to end.
#[test]
fn embedded_shuffled_vm_matches_node() {
    for src in PROGRAMS {
        // kumo parses as a module (strict mode); match that natively
        let native = node_run(&format!("\"use strict\";\n{src}"));
        for seed in [0u64, 3, 2024, u64::MAX] {
            let embedded = compile_embedded_source("p.js", src, seed).unwrap();
            assert_eq!(native, node_run(&embedded), "seed {seed} output differs for {src:?}");
        }
    }
}
