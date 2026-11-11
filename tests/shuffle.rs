//! Opcode shuffling is behaviour-preserving, end to end through `tools/build.mjs`: bytecode
//! compiled under a shuffled build's opcode map disassembles to the same listing as the
//! canonical build, and a bundled shuffled VM + program runs to the same output as Node.

use kumojs::disasm::{disassemble, disassemble_with};
use kumojs::opcode::OpcodeMap;
use kumojs::Compiler;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAMS: &[&str] = &[
    "window.console.log(1 + 2 * 3, [3, 1, 2].sort().join(','));",
    "function fib(n){ return n < 2 ? n : fib(n-1)+fib(n-2); } window.console.log(fib(12));",
    "class A { constructor(v){ this.v = v; } get d(){ return this.v*2; } } window.console.log(new A(21).d);",
    "let t = 0; for (let x of [1,2,3,4]) t += x*x; window.console.log(t);",
    // captured `let`s whose boxes start uninitialised (patched after emitting)
    "switch (1) { case 1: let x = 5; window.console.log((() => x)()); }",
    "function f() { return y; } let y = 2; window.console.log(f());",
];

/// A scratch directory for one test, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kumo_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // scripts are written against `window`; Node only has `globalThis`
        std::fs::write(dir.join("window.cjs"), "globalThis.window = globalThis;\n").unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `node tools/build.mjs <args>` from the repo root, with kumoc from this build.
fn build_tool(args: &[&str]) {
    let out = Command::new("node")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["--no-warnings", "tools/build.mjs"])
        .args(args)
        .env("KUMOC", env!("CARGO_BIN_EXE_kumoc"))
        .output()
        .expect("run node");
    assert!(
        out.status.success(),
        "build.mjs {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run a script in Node (with `window` defined) and return its stdout.
fn node_run(tmp: &TempDir, path: &Path) -> String {
    let out = Command::new("node")
        .arg("--require")
        .arg(tmp.0.join("window.cjs"))
        .arg(path)
        .output()
        .expect("run node");
    assert!(
        out.status.success(),
        "node failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn seeded_bytecode_disassembles_canonically() {
    let tmp = TempDir::new("shuffle_disasm");
    for seed in [0u32, 1, 42, 7777] {
        let map_path = tmp.0.join(format!("{seed}.json"));
        let vm_path = tmp.0.join(format!("{seed}.js"));
        build_tool(&[
            "vm",
            "--seed",
            &seed.to_string(),
            "--out",
            vm_path.to_str().unwrap(),
            "--map",
            map_path.to_str().unwrap(),
        ]);
        let map = OpcodeMap::from_json(&std::fs::read_to_string(&map_path).unwrap()).unwrap();
        assert!(!map.is_identity(), "seed {seed} didn't shuffle");
        for src in PROGRAMS {
            let canonical = Compiler::new().compile_source("p.js", src).unwrap();
            let shuffled = Compiler::new()
                .with_opcode_map(map.clone())
                .compile_source("p.js", src)
                .unwrap();
            assert_ne!(canonical, shuffled);
            assert_eq!(
                disassemble(&canonical).unwrap(),
                disassemble_with(&shuffled, &map).unwrap(),
                "seed {seed} listing differs for {src:?}"
            );
        }
    }
}

/// The bundled VM + program for a shuffled build runs to the same output as the program
/// does natively in Node.
#[test]
fn bundled_shuffled_program_matches_node() {
    let tmp = TempDir::new("shuffle_bundle");
    for (i, src) in PROGRAMS.iter().enumerate() {
        // kumo parses as a module (strict mode); match that natively
        let native = tmp.0.join(format!("{i}_native.js"));
        std::fs::write(&native, format!("\"use strict\";\n{src}")).unwrap();
        let expected = node_run(&tmp, &native);

        let input = tmp.0.join(format!("{i}.js"));
        std::fs::write(&input, src).unwrap();
        for seed in [0u32, 3, 2024, u32::MAX] {
            let bundled = tmp.0.join(format!("{i}_{seed}.bundle.js"));
            build_tool(&[
                "bundle",
                input.to_str().unwrap(),
                "--seed",
                &seed.to_string(),
                "--out",
                bundled.to_str().unwrap(),
            ]);
            assert_eq!(
                expected,
                node_run(&tmp, &bundled),
                "seed {seed} output differs for {src:?}"
            );
        }
    }
}
