//! Bytecode disassembler: a readable listing of compiled code, for debugging and the
//! bytecode snapshot tests. Each line is `offset  Name operands`; jump targets are shown as
//! absolute offsets in the same function, and nested functions are listed after a header.

use std::fmt::Write;

/// How an instruction's operand bytes are laid out.
#[derive(Clone, Copy)]
enum Operand {
    U8,
    U16,
    U32,
    F64,
    /// u32 byte length, then WTF-8 bytes
    Str,
    /// u32 forward offset from the end of this operand
    Jump,
    /// u32 backward offset (`Loop`): the target is this operand's end minus it
    Back,
    /// u32 forward offset from the end of the whole instruction
    JumpFromEnd,
}

use Operand::*;

/// (opcode, name, operands). Kept in step with `Operation::get_opcode`; the snapshot tests
/// disassemble every fixture, so a missing or wrong entry fails them.
const OPS: &[(u8, &str, &[Operand])] = &[
    (0x01, "LoadString", &[Str]),
    (0x02, "LoadFloat64", &[F64]),
    (0x03, "Bool", &[U8]),
    (0x04, "Pop", &[]),
    (0x05, "Null", &[]),
    (0x06, "Regex", &[Str, Str]),
    (0x07, "Undefined", &[]),
    (0x08, "Return", &[]),
    (0x0b, "UInt8", &[U8]),
    (0x0c, "UInt16", &[U16]),
    (0x0d, "UInt32", &[U32]),
    (0x0e, "UInt64", &[U32, U32]),
    (0x0f, "Call", &[]),
    (0x10, "GetProperty", &[]),
    (0x11, "SetProperty", &[]),
    (0x12, "EqEq", &[]),
    (0x13, "NotEq", &[]),
    (0x14, "EqEqEq", &[]),
    (0x15, "NotEqEq", &[]),
    (0x16, "Lt", &[]),
    (0x17, "LtEq", &[]),
    (0x18, "Gt", &[]),
    (0x19, "GtEq", &[]),
    (0x1a, "LShift", &[]),
    (0x1b, "RShift", &[]),
    (0x1c, "ZeroFillRShift", &[]),
    (0x1d, "Add", &[]),
    (0x1e, "Sub", &[]),
    (0x1f, "Mul", &[]),
    (0x20, "Div", &[]),
    (0x21, "Mod", &[]),
    (0x22, "BitOr", &[]),
    (0x23, "BitXor", &[]),
    (0x24, "BitAnd", &[]),
    (0x27, "In", &[]),
    (0x28, "InstanceOf", &[]),
    (0x29, "Exp", &[]),
    (0x2b, "GetLocal", &[U8]),
    (0x2c, "SetLocal", &[U8]),
    (0x2d, "UnaryMinus", &[]),
    (0x2e, "UnaryPlus", &[]),
    (0x2f, "LogicalNot", &[]),
    (0x30, "BitwiseNot", &[]),
    (0x31, "TypeOf", &[]),
    (0x32, "Void", &[]),
    (0x33, "Delete", &[]),
    (0x34, "Array", &[U32]),
    (0x35, "Object", &[U32]),
    (0x36, "Dup", &[]),
    // 0x37 / 0x4b Function: decoded specially
    (0x38, "GetUpvalue", &[U8]),
    (0x39, "SetUpvalue", &[U8]),
    (0x3a, "Jump", &[Jump]),
    (0x3b, "JumpIfFalse", &[Jump]),
    (0x3c, "JumpIfTrue", &[Jump]),
    (0x3d, "JumpIfNotNullish", &[Jump]),
    (0x3e, "CallMethod", &[]),
    (0x3f, "This", &[]),
    (0x40, "HostGlobal", &[U8]),
    (0x41, "Loop", &[Back]),
    (0x42, "Dup2", &[]),
    (0x43, "Callee", &[]),
    (0x44, "Arguments", &[]),
    (0x45, "New", &[]),
    (0x46, "Throw", &[]),
    (0x47, "TryStart", &[Jump]),
    (0x48, "TryEnd", &[]),
    (0x49, "ForInKeys", &[]),
    (0x4a, "Hole", &[]),
    (0x4c, "GetBox", &[U8]),
    (0x4d, "SetBox", &[U8]),
    (0x4e, "NewBox", &[U8]),
    (0x4f, "BoxLocal", &[U8]),
    (0x50, "RenewBox", &[U8]),
    (0x51, "ArrayAppend", &[]),
    (0x52, "ArraySpread", &[]),
    (0x53, "SpreadArgs", &[]),
    (0x54, "RestArgs", &[U8]),
    (0x55, "IterTake", &[U32]),
    (0x56, "ArraySliceFrom", &[U32]),
    (0x57, "RequireObjectCoercible", &[]),
    (0x58, "ObjectRest", &[]),
    (0x59, "Template", &[U32]),
    // 0x5a TemplateObject: decoded specially
    (0x5b, "DefineField", &[]),
    (0x5c, "DefineAccessor", &[U8]),
    (0x5d, "ObjectSpread", &[]),
    (0x5e, "SetFunctionName", &[U8]),
    (0x5f, "MakeClass", &[U8]),
    (0x60, "DefineMethod", &[U8]),
    (0x61, "ClassField", &[]),
    (0x62, "StaticField", &[]),
    (0x63, "StaticBlock", &[]),
    (0x64, "SuperCall", &[]),
    (0x65, "SuperGet", &[]),
    (0x66, "SuperSet", &[]),
    (0x67, "NewTarget", &[]),
    (0x68, "NewPrivateName", &[U8]),
    (0x69, "PrivateGet", &[]),
    (0x6a, "PrivateSet", &[]),
    (0x6b, "PrivateIn", &[]),
    (0x6c, "DefinePrivateMethod", &[U8]),
    (0x6d, "LoadBigInt", &[Str]),
    (0x6e, "Increment", &[]),
    (0x6f, "Decrement", &[]),
    (0x70, "ToNumeric", &[]),
    (0x71, "GetIterator", &[]),
    (0x72, "IterNext", &[U8, Jump]),
    (0x73, "IterClose", &[U8, U8]),
    (0x74, "JumpIfNullish", &[Jump]),
    (0x75, "InitialYield", &[]),
    (0x76, "Yield", &[U8, JumpFromEnd, JumpFromEnd, JumpFromEnd]),
    (0x77, "Await", &[]),
    (0x78, "GetAsyncIterator", &[]),
    (0x79, "DelegateCall", &[U8, U8, Jump]),
    (0x7a, "DelegateResult", &[U8, U8, JumpFromEnd, JumpFromEnd]),
    (0x7b, "AsyncIterNext", &[U8]),
    (0x7c, "IterResult", &[U8, Jump]),
    (0x7d, "AsyncIterClose", &[U8, U8, Jump]),
    (0x7e, "IterCloseCheck", &[U8]),
    (0x7f, "ToPropertyKey", &[]),
    (0x80, "InitInstance", &[]),
    (0x81, "SuperCallFinish", &[]),
    (0x82, "InitField", &[U32, U8]),
    (0x83, "ClassInit", &[]),
    (0x84, "GetSlotIndex", &[U8, U32]),
    (0x85, "GetSlotProp", &[U8, Str]),
    (0x86, "NewBoxTdz", &[U8]),
    (0x87, "Tdz", &[U8]),
    (0x88, "CheckTdz", &[]),
    (0x89, "ThrowTdz", &[]),
];

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

/// Disassemble a compiled script (or a function's code).
pub fn disassemble(code: &[u8]) -> Result<String, String> {
    let mut out = String::new();
    listing(code, 0, &mut out)?;
    // no trailing spaces (operand-less instructions)
    Ok(out.lines().map(str::trim_end).collect::<Vec<_>>().join("\n") + "\n")
}

fn listing(code: &[u8], depth: usize, out: &mut String) -> Result<(), String> {
    let indent = "    ".repeat(depth);
    let mut r = Reader { bytes: code, pos: 0 };
    while r.pos < code.len() {
        let at = r.pos;
        let opcode = r.u8()?;
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
                listing(body, depth + 1, out)?;
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
                let (_, name, operands) = OPS
                    .iter()
                    .find(|(op, ..)| *op == opcode)
                    .ok_or_else(|| format!("unknown opcode 0x{opcode:02x} at {at:04x}"))?;
                // offsets from the instruction's end are resolved once it's all read
                let mut parts = Vec::new();
                let mut from_end = Vec::new();
                for operand in *operands {
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
