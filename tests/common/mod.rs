#![allow(dead_code)]

use kumojs::Compiler;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn compile(source: &str) -> Vec<u8> {
    Compiler::new()
        .compile_source("test.js", source)
        .unwrap_or_else(|e| panic!("compile failed: {e}"))
}

/// Write bytecode to a unique temp file and return its path.
pub fn write_bytecode(bytecode: &[u8]) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "kumo_test_{}_{}.kumo",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytecode).expect("failed to write bytecode");
    path
}

/// Compile `source`, run it in the VM via `test_runner.js` and return stdout.
/// `verification_script` is eval'd by the runner after the program finishes.
pub fn run_vm(source: &str, verification_script: Option<&str>) -> String {
    let bytecode_path = write_bytecode(&compile(source));

    let mut cmd = Command::new("node");
    cmd.arg("src/vm/test_runner.js").arg(&bytecode_path);
    if let Some(script) = verification_script {
        cmd.arg(script);
    }
    let output = cmd
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to execute node process");
    let _ = std::fs::remove_file(&bytecode_path);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!("node process failed:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}");
    }
    stdout
}

/// Assert the program's stdout (console output, then the value left on top of the
/// VM stack as printed by the runner) matches `expected`.
pub fn run_test(source: &str, expected: &str) {
    let stdout = run_vm(source, None);
    assert_eq!(stdout.trim(), expected);
}
