//! Emitting bytecode: opcodes and their operands, jumps, and small instruction helpers.

use super::prelude::*;

/// Push a string constant.
pub(super) fn load_str(s: &str) -> Operation {
    Operation::LoadString(s.as_bytes().to_vec())
}

impl FnCompiler<'_> {
    /// The byte `op` is emitted as, after any opcode renumbering.
    pub(super) fn op_byte(&self, op: &Operation) -> u8 {
        self.opcode_map.to_byte[op.get_opcode() as usize]
    }

    pub(super) fn emit_op(&mut self, op: Operation) {
        self.bytecode.push(self.op_byte(&op));
        self.emit_operands(op);
    }

    fn emit_operands(&mut self, op: Operation) {
        let out = &mut self.bytecode;
        match op {
            Operation::LoadString(bytes) => emit_string(out, &bytes),
            Operation::LoadBigInt(digits) => emit_string(out, digits.as_bytes()),
            Operation::LoadFloat64(n) => out.extend_from_slice(&n.to_le_bytes()),
            Operation::Bool(b) => out.push(b as u8),
            Operation::Regex(exp, flags) => {
                emit_string(out, exp.as_bytes());
                emit_string(out, flags.as_bytes());
            }
            Operation::UInt8(n) => out.push(n),
            Operation::UInt16(n) => out.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt32(n) => out.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt64(n) => {
                // can't easily read 64-bit integers in JS, so it's two 32-bit halves
                out.extend_from_slice(&(n as u32).to_le_bytes());
                out.extend_from_slice(&((n >> 32) as u32).to_le_bytes());
            }
            Operation::GetLocal(slot)
            | Operation::SetLocal(slot)
            | Operation::GetBox(slot)
            | Operation::SetBox(slot)
            | Operation::NewBox(slot)
            | Operation::BoxLocal(slot)
            | Operation::RenewBox(slot)
            | Operation::NewBoxTdz(slot)
            | Operation::Tdz(slot)
            | Operation::RestArgs(slot)
            | Operation::GetUpvalue(slot)
            | Operation::SetUpvalue(slot) => out.push(slot),
            Operation::GetSlotIndex(slot, index) => {
                out.push(slot);
                out.extend_from_slice(&index.to_le_bytes());
            }
            Operation::GetSlotProp(slot, name) => {
                out.push(slot);
                emit_string(out, &name);
            }
            Operation::HostGlobal(global, typeof_safe) => {
                let flag = if typeof_safe { HOST_GLOBAL_TYPEOF } else { 0 };
                out.push(global as u8 | flag);
            }
            Operation::Array(n)
            | Operation::Object(n)
            | Operation::IterTake(n)
            | Operation::ArraySliceFrom(n)
            | Operation::Template(n)
            | Operation::Jump(n)
            | Operation::JumpIfFalse(n)
            | Operation::JumpIfTrue(n)
            | Operation::JumpIfNotNullish(n)
            | Operation::JumpIfNullish(n)
            | Operation::Loop(n)
            | Operation::TryStart(n) => out.extend_from_slice(&n.to_le_bytes()),
            Operation::IterNext(slot, offset) => {
                out.push(slot);
                out.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::IterClose(slot, exit) => {
                out.push(slot);
                out.push(exit as u8);
            }
            Operation::Function(info) => emit_fn_info(out, info),
            Operation::SetFunctionName(flavor) | Operation::DefineMethod(flavor) => {
                out.push(flavor as u8)
            }
            Operation::DefineAccessor(accessor) => out.push(accessor as u8),
            Operation::DefinePrivateMethod(flavor, is_static) => {
                let flag = if is_static { PRIVATE_METHOD_STATIC } else { 0 };
                out.push(flavor as u8 | flag);
            }
            Operation::MakeClass(derived) => out.push(derived as u8),
            Operation::NewPrivateName(kind) => out.push(kind as u8),
            Operation::InitField(index, name_it) => {
                out.extend_from_slice(&index.to_le_bytes());
                out.push(name_it as u8);
            }
            Operation::TemplateObject(pieces) => {
                out.extend_from_slice(&(pieces.len() as u32).to_le_bytes());
                for (cooked, raw) in pieces {
                    out.push(cooked.is_some() as u8);
                    if let Some(cooked) = cooked {
                        emit_string(out, &cooked);
                    }
                    emit_string(out, &raw);
                }
            }
            _ => {}
        }
    }

    /// Emit a forward jump with a placeholder offset; returns the offset's position for
    /// `patch_jump`.
    pub(super) fn emit_jump(&mut self, op: Operation) -> usize {
        self.emit_op(op);
        self.bytecode.len() - 4
    }

    /// Point a jump emitted by `emit_jump` at the current end of the bytecode.
    pub(super) fn patch_jump(&mut self, pos: usize) {
        let offset = (self.bytecode.len() - (pos + 4)) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// Emit a backward jump to `start`.
    pub(super) fn emit_loop(&mut self, start: usize) {
        // the VM lands on `start` after reading the 4 byte operand
        let offset = (self.bytecode.len() + 1 + 4 - start) as u32;
        self.emit_op(Operation::Loop(offset));
    }

    /// Store the top of the stack in a plain slot and drop it.
    pub(super) fn emit_set_local_pop(&mut self, slot: u8) {
        self.emit_op(Operation::SetLocal(slot));
        self.emit_op(Operation::Pop);
    }

    /// Call what's on the stack: [obj, f, ...args, argc] for a method, else [f, ...args, argc].
    pub(super) fn emit_call(&mut self, is_method: bool) {
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
    }

    /// Push the value of a resolved variable.
    pub(super) fn emit_load_ref(&mut self, var: VarRef) {
        self.emit_op(match var {
            VarRef::Local(slot) => Operation::GetLocal(slot),
            VarRef::Box(slot) => Operation::GetBox(slot),
            VarRef::Upvalue(index) => Operation::GetUpvalue(index),
        });
    }

    /// Store the top of the stack into a resolved variable, leaving it on the stack.
    pub(super) fn emit_store_ref(&mut self, var: VarRef) {
        self.emit_op(match var {
            VarRef::Local(slot) => Operation::SetLocal(slot),
            VarRef::Box(slot) => Operation::SetBox(slot),
            VarRef::Upvalue(index) => Operation::SetUpvalue(index),
        });
    }
}

/// Strings are a u32 byte length followed by WTF-8 bytes.
fn emit_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// length, params, flags, name, captures (count, then (from_local, index) pairs), then
/// the code's u32 length and the code.
fn emit_fn_info(out: &mut Vec<u8>, info: FnInfo) {
    out.push(info.length);
    out.push(info.params);
    let async_flag = if info.is_async { FN_ASYNC } else { 0 };
    out.push(info.kind as u8 | async_flag);
    emit_string(out, info.name.as_bytes());
    out.push(info.captures.len() as u8);
    for capture in info.captures {
        let (from_local, index) = match capture {
            Capture::Local(slot) => (true, slot),
            Capture::Upvalue(index) => (false, index),
        };
        out.push(from_local as u8);
        out.push(index);
    }
    out.extend_from_slice(&(info.code.len() as u32).to_le_bytes());
    out.extend_from_slice(&info.code);
}
