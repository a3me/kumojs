//! Emitting bytecode: jumps, loops, opcodes and their operands.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // emitting
    // ---------------------------------------------------------------------------------

    /// Emit a forward jump with a placeholder offset; returns the offset's position for `patch_jump`.
    pub(crate) fn emit_jump(&mut self, op: Operation) -> usize {
        self.emit_op(op);
        self.bytecode.len() - 4
    }

    /// Point a jump emitted by `emit_jump` at the current end of the bytecode.
    pub(crate) fn patch_jump(&mut self, pos: usize) {
        let offset = (self.bytecode.len() - (pos + 4)) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// Emit a backward jump to `start`.
    pub(crate) fn emit_loop(&mut self, start: usize) {
        // the VM lands on `start` after reading the 4 byte operand
        let offset = (self.bytecode.len() + 1 + 4 - start) as u32;
        self.emit_op(Operation::Loop(offset));
    }

    pub(crate) fn emit_op(&mut self, op: Operation) {
        self.bytecode
            .push(self.opcode_map.to_byte[op.get_opcode() as usize]);
        match op {
            Operation::LoadString(bytes) => self.emit_string(&bytes),
            Operation::LoadBigInt(digits) => self.emit_string(digits.as_bytes()),
            Operation::IterNext(slot, offset) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::IterClose(slot, mode) => {
                self.bytecode.push(slot);
                self.bytecode.push(mode);
            }
            Operation::GetSlotIndex(slot, index) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&index.to_le_bytes());
            }
            Operation::GetSlotProp(slot, name) => {
                self.bytecode.push(slot);
                self.emit_string(&name);
            }
            Operation::InitField(index, name_it) => {
                self.bytecode.extend_from_slice(&index.to_le_bytes());
                self.bytecode.push(name_it);
            }
            Operation::Yield(raw) => {
                self.bytecode.push(raw);
                self.bytecode.extend_from_slice(&[0; 12]);
            }
            Operation::DelegateCall(iterator, mode, offset) => {
                self.bytecode.push(iterator);
                self.bytecode.push(mode);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::DelegateResult(mode, is_async) => {
                self.bytecode.push(mode);
                self.bytecode.push(is_async);
                self.bytecode.extend_from_slice(&[0; 8]);
            }
            Operation::AsyncIterNext(slot) | Operation::IterCloseCheck(slot) => {
                self.bytecode.push(slot)
            }
            Operation::IterResult(slot, offset) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::AsyncIterClose(slot, mode, offset) => {
                self.bytecode.push(slot);
                self.bytecode.push(mode);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::LoadFloat64(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Bool(b) => self.bytecode.push(b as u8),
            Operation::Regex(exp, flags) => {
                self.emit_string(exp.as_bytes());
                self.emit_string(flags.as_bytes());
            }
            Operation::UInt8(n) => self.bytecode.push(n),
            Operation::UInt16(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt32(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt64(n) => {
                // can't easily emit 64-bit integers in JS, so we split into two 32-bit integers
                let low = n as u32;
                let high = (n >> 32) as u32;
                self.bytecode.extend_from_slice(&low.to_le_bytes());
                self.bytecode.extend_from_slice(&high.to_le_bytes());
            }
            Operation::Function(FnInfo {
                length,
                params,
                kind,
                coroutine,
                name,
                captures,
                code,
                ..
            }) => {
                self.bytecode.push(length);
                self.bytecode.push(params);
                // kind, | 8 generator, | 16 async
                self.bytecode.push(
                    kind as u8
                        | (coroutine.is_generator() as u8) << 3
                        | (coroutine.is_async() as u8) << 4,
                );
                self.emit_string(name.as_bytes());
                // captures: count, then (from_local, index) pairs
                self.bytecode.push(captures.len() as u8);
                for (from_local, index) in captures {
                    self.bytecode.push(from_local as u8);
                    self.bytecode.push(index);
                }
                let code_len = code.len() as u32;
                self.bytecode.extend_from_slice(&code_len.to_le_bytes());
                self.bytecode.extend_from_slice(&code);
            }
            Operation::GetUpvalue(index) | Operation::SetUpvalue(index) => {
                self.bytecode.push(index)
            }
            Operation::GetBox(slot)
            | Operation::SetBox(slot)
            | Operation::NewBox(slot)
            | Operation::BoxLocal(slot)
            | Operation::RenewBox(slot)
            | Operation::NewBoxTdz(slot)
            | Operation::Tdz(slot)
            | Operation::RestArgs(slot) => self.bytecode.push(slot),
            Operation::HostGlobal(id) => self.bytecode.push(id),
            Operation::DefineAccessor(n)
            | Operation::SetFunctionName(n)
            | Operation::MakeClass(n)
            | Operation::DefineMethod(n)
            | Operation::NewPrivateName(n)
            | Operation::DefinePrivateMethod(n) => self.bytecode.push(n),
            Operation::TemplateObject(pieces) => {
                self.bytecode
                    .extend_from_slice(&(pieces.len() as u32).to_le_bytes());
                for (cooked, raw) in pieces {
                    self.bytecode.push(cooked.is_some() as u8);
                    if let Some(cooked) = cooked {
                        self.emit_string(&cooked);
                    }
                    self.emit_string(&raw);
                }
            }
            Operation::GetLocal(index) | Operation::SetLocal(index) => self.bytecode.push(index),
            Operation::Array(n)
            | Operation::Object(n)
            | Operation::IterTake(n)
            | Operation::ArraySliceFrom(n)
            | Operation::Template(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::Jump(offset)
            | Operation::JumpIfFalse(offset)
            | Operation::JumpIfTrue(offset)
            | Operation::JumpIfNotNullish(offset)
            | Operation::JumpIfNullish(offset)
            | Operation::Loop(offset)
            | Operation::TryStart(offset) => {
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            _ => {}
        }
    }

    /// Strings are a u32 byte length followed by WTF-8 bytes.
    pub(crate) fn emit_string(&mut self, bytes: &[u8]) {
        self.bytecode
            .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.bytecode.extend_from_slice(bytes);
    }
}
