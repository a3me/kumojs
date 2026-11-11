use std::process::Command;

// src/vm/vm.js is generated from src/vm/vm.ts (types stripped by Node).
fn main() {
    println!("cargo:rerun-if-changed=src/vm/vm.ts");
    println!("cargo:rerun-if-changed=tools/build-vm.mjs");
    let status = Command::new("node")
        .args(["--no-warnings", "tools/build-vm.mjs"])
        .status()
        .expect("node (v23.2+) is required to build src/vm/vm.js from vm.ts");
    assert!(status.success(), "tools/build-vm.mjs failed");
}
