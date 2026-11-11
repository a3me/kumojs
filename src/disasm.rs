//! Bytecode disassembler: a readable listing of compiled code, for debugging and the
//! bytecode snapshot tests. Each line is `offset  Name operands`; jump targets are shown as
//! absolute offsets in the same function, and nested functions are listed after a header.

use crate::opcode::{Operand::*, OpcodeMap};
use crate::OPCODES;
use std::fmt::Write;

struct Reader<'b> {
    bytes: &'b [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos + n;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| format!("truncated at {:04x}", self.pos))?;
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        // WTF-8 lone surrogates show as replacement characters
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
}

/// Disassemble a compiled script (or a function's code) with the canonical opcode bytes.
pub fn disassemble(code: &[u8]) -> Result<String, String> {
    disassemble_with(code, &OpcodeMap::identity())
}

/// Disassemble bytecode whose opcodes were renumbered by `map` (see `OpcodeMap`). Names
/// are always the canonical ones, so a listing reads the same whatever the numbering.
pub fn disassemble_with(code: &[u8], map: &OpcodeMap) -> Result<String, String> {
    let mut out = String::new();
    listing(code, 0, map, &mut out)?;
    // no trailing spaces (operand-less instructions)
    Ok(out.lines().map(str::trim_end).collect::<Vec<_>>().join("\n") + "\n")
}

fn listing(code: &[u8], depth: usize, map: &OpcodeMap, out: &mut String) -> Result<(), String> {
    let indent = "    ".repeat(depth);
    let mut r = Reader { bytes: code, pos: 0 };
    while r.pos < code.len() {
        let at = r.pos;
        // read the raw byte, then undo any renumbering to get the canonical opcode
        let opcode = map.from_byte[r.u8()? as usize];
        let _ = write!(out, "{indent}{at:04x}  ");
        match opcode {
            0x37 | 0x4b => {
                let length = r.u8()?;
                let params = r.u8()?;
                let flags = r.u8()?;
                let name = r.string()?;
                let captures = (0..r.u8()?)
                    .map(|_| Ok(format!("{}{}", if r.u8()? == 1 { "L" } else { "U" }, r.u8()?)))
                    .collect::<Result<Vec<_>, String>>()?;
                let len = r.u32()? as usize;
                let body = r.take(len)?;
                let kind = ["normal", "arrow", "method", "class", "derived class"]
                    .get((flags & 7) as usize)
                    .copied()
                    .unwrap_or("?");
                let mut extra = String::new();
                if flags & 8 != 0 {
                    extra.push_str(" generator");
                }
                if flags & 16 != 0 {
                    extra.push_str(" async");
                }
                let wrapped = if opcode == 0x4b { " internal" } else { "" };
                let _ = writeln!(
                    out,
                    "Function {name:?} ({kind}{extra}{wrapped}, length {length}, params {params}, captures [{}])",
                    captures.join(" ")
                );
                listing(body, depth + 1, map, out)?;
            }
            0x5a => {
                let count = r.u32()?;
                let mut pieces = Vec::new();
                for _ in 0..count {
                    let cooked = if r.u8()? == 1 { Some(r.string()?) } else { None };
                    let raw = r.string()?;
                    pieces.push(format!("{cooked:?}/{raw:?}"));
                }
                let _ = writeln!(out, "TemplateObject [{}]", pieces.join(", "));
            }
            _ => {
                let info = OPCODES
                    .iter()
                    .find(|o| o.code == opcode)
                    .ok_or_else(|| format!("unknown opcode 0x{opcode:02x} at {at:04x}"))?;
                let name = info.name;
                // offsets from the instruction's end are resolved once it's all read
                let mut parts = Vec::new();
                let mut from_end = Vec::new();
                for operand in info.operands {
                    let part = match operand {
                        U8 => r.u8()?.to_string(),
                        U16 => r.u16()?.to_string(),
                        U32 => r.u32()?.to_string(),
                        F64 => format!("{:?}", r.f64()?),
                        Str => format!("{:?}", r.string()?),
                        Jump => {
                            let offset = r.u32()? as usize;
                            format!("-> {:04x}", r.pos + offset)
                        }
                        Back => {
                            let offset = r.u32()? as usize;
                            format!("-> {:04x}", r.pos.saturating_sub(offset))
                        }
                        JumpFromEnd => {
                            from_end.push((parts.len(), r.u32()? as usize));
                            String::new()
                        }
                    };
                    parts.push(part);
                }
                for (i, offset) in from_end {
                    parts[i] = format!("-> {:04x}", r.pos + offset);
                }
                let _ = writeln!(out, "{name} {}", parts.join(" ").trim_end());
            }
        }
    }
    Ok(())
}
