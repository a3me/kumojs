//! Bytecode snapshot tests: each `tests/snapshots/*.js` is compiled and disassembled, and
//! the listing must match the `.txt` next to it. Run with `KUMO_UPDATE_SNAPSHOTS=1` to
//! write the listings after an intended compiler change (then review the diff).

use kumojs::{disasm::disassemble, Compiler};
use std::path::{Path, PathBuf};

fn js_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            js_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "js") {
            out.push(path);
        }
    }
}

fn listing(path: &Path) -> String {
    let source = std::fs::read_to_string(path).unwrap();
    let name = path.file_name().unwrap().to_string_lossy();
    let bytecode = Compiler::new()
        .compile_source(&name, &source)
        .unwrap_or_else(|e| panic!("{}: compile failed: {e}", path.display()));
    disassemble(&bytecode).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn bytecode_snapshots() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let update = std::env::var_os("KUMO_UPDATE_SNAPSHOTS").is_some();
    let mut files = Vec::new();
    js_files(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no snapshot sources in {}",
        root.display()
    );

    let mut mismatched = Vec::new();
    for path in files {
        let actual = listing(&path);
        let expected_path = path.with_extension("txt");
        if update {
            std::fs::write(&expected_path, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
        if expected != actual {
            mismatched.push(format!(
                "{}\n--- expected\n{expected}\n--- actual\n{actual}",
                path.display()
            ));
        }
    }
    assert!(
        mismatched.is_empty(),
        "bytecode changed (KUMO_UPDATE_SNAPSHOTS=1 to accept):\n{}",
        mismatched.join("\n")
    );
}

/// Every fixture and benchmark disassembles cleanly: catches opcodes the disassembler
/// doesn't know (or decodes with the wrong operand layout).
#[test]
fn disassembler_covers_all_bytecode() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    js_files(&root.join("tests/fixtures"), &mut files);
    js_files(&root.join("bench/cases"), &mut files);
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        let Ok(bytecode) = Compiler::new().compile_source("input.js", &source) else {
            continue; // skipped fixtures may not compile
        };
        if let Err(e) = disassemble(&bytecode) {
            panic!("{}: {e}", path.display());
        }
    }
}
