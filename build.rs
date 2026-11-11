use std::process::Command;

// src/vm/vm.js is built from src/vm/vm.ts (types stripped, opcodes inlined) by tools/build.mjs.
fn main() {
    for path in ["src/vm/vm.ts", "src/vm/opcodes.d.ts", "tools/build.mjs"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let status = Command::new("node")
        .args(["--no-warnings", "tools/build.mjs", "vm"])
        .status()
        .expect("node (v23.2+) is required to build src/vm/vm.js from vm.ts");
    assert!(status.success(), "tools/build.mjs failed");
}
