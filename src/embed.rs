//! Produces a standalone JS file: the VM with its opcode `switch` renumbered to match an
//! `OpcodeMap`, optionally with a compiled program embedded so `node out.js` runs it.
//!
//! The checked-in `vm/vm.js` holds the canonical opcode numbers (so it's readable and the
//! snapshots are stable). When a build shuffles the opcodes, `generate_vm` rewrites the
//! dispatch `case` labels to match, and drops the opcode-name comments so the output
//! doesn't give the numbering away.

use crate::opcode::OpcodeMap;
use crate::{CompileError, Compiler};
use std::path::Path;

/// The canonical VM source. Its dispatch `case 0xNN:` labels are the canonical opcode bytes.
pub const VM_SOURCE: &str = include_str!("vm/vm.js");

/// The VM source with its dispatch `case` labels renumbered for `map`, and the
/// `// Operation::…` comments removed.
pub fn generate_vm(map: &OpcodeMap) -> String {
    let mut out = String::with_capacity(VM_SOURCE.len());
    for line in VM_SOURCE.lines() {
        if line.trim_start().starts_with("// Operation::") {
            continue;
        }
        rewrite_cases(line, map, &mut out);
        out.push('\n');
    }
    out
}

/// A standalone script: the VM for `map`, plus `bytecode` embedded and run on load.
pub fn embed(map: &OpcodeMap, bytecode: &[u8]) -> String {
    let mut js = generate_vm(map);
    js.push_str("\n;(function () {\n  var code = [");
    for (i, b) in bytecode.iter().enumerate() {
        if i > 0 {
            js.push(',');
        }
        js.push_str(&b.to_string());
    }
    js.push_str("];\n  new VM([code]).run();\n})();\n");
    js
}

/// Compile `source` under `seed`'s numbering and embed it in a matching VM.
pub fn compile_embedded_source(name: &str, source: &str, seed: u64) -> Result<String, CompileError> {
    let map = OpcodeMap::from_seed(seed);
    let bytecode = Compiler::new()
        .with_opcode_map(map.clone())
        .compile_source(name, source)?;
    Ok(embed(&map, &bytecode))
}

/// Compile the file at `path` under `seed`'s numbering and embed it in a matching VM.
pub fn compile_embedded_file(path: &Path, seed: u64) -> Result<String, CompileError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| CompileError::LoadFileError(e.to_string()))?;
    let name = path.to_string_lossy();
    compile_embedded_source(&name, &source, seed)
}

/// Append each `case 0xNN` in `line`, with NN renumbered by `map`, to `out`.
fn rewrite_cases(line: &str, map: &OpcodeMap, out: &mut String) {
    const MARKER: &str = "case 0x";
    let mut rest = line;
    while let Some(idx) = rest.find(MARKER) {
        out.push_str(&rest[..idx]);
        out.push_str(MARKER);
        let after = &rest[idx + MARKER.len()..];
        let hex_len = after.chars().take_while(char::is_ascii_hexdigit).count();
        let (hex, tail) = after.split_at(hex_len);
        let code = u8::from_str_radix(hex, 16).expect("case label is a byte");
        out.push_str(&format!("{:02x}", map.to_byte[code as usize]));
        rest = tail;
    }
    out.push_str(rest);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every dispatch `case` in vm.js is a known opcode, and every opcode has a case — so the
    /// interpreter and the compiler can't disagree about what bytes exist.
    #[test]
    fn vm_cases_match_opcodes() {
        let mut cases = HashSet::new();
        for line in VM_SOURCE.lines() {
            let mut rest = line;
            while let Some(idx) = rest.find("case 0x") {
                let after = &rest[idx + 7..];
                let len = after.chars().take_while(char::is_ascii_hexdigit).count();
                cases.insert(u8::from_str_radix(&after[..len], 16).unwrap());
                rest = &after[len..];
            }
        }
        let opcodes: HashSet<u8> = crate::OPCODES.iter().map(|o| o.code).collect();
        let missing_case: Vec<_> = opcodes.difference(&cases).collect();
        let unknown_case: Vec<_> = cases.difference(&opcodes).collect();
        assert!(missing_case.is_empty(), "opcodes with no vm.js case: {missing_case:02x?}");
        assert!(unknown_case.is_empty(), "vm.js cases with no opcode: {unknown_case:02x?}");
    }

    #[test]
    fn identity_generate_only_drops_comments() {
        let generated = generate_vm(&OpcodeMap::identity());
        // same case labels as the source (identity doesn't renumber)
        assert!(generated.contains("case 0x01:"));
        assert!(generated.contains("case 0x4b:"));
        assert!(!generated.contains("// Operation::"));
    }

    #[test]
    fn seeded_generate_renumbers_cases() {
        let map = OpcodeMap::from_seed(123);
        let generated = generate_vm(&map);
        let mapped = format!("case 0x{:02x}:", map.to_byte[0x01]);
        assert!(generated.contains(&mapped));
    }
}
