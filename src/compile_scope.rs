//! Scopes, locals, slots, upvalues and the temporal dead zone.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // scopes and variables
    // ---------------------------------------------------------------------------------

    pub(crate) fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    /// Leave a block: its names go out of scope (their slots are not reused).
    pub(crate) fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
        let keep = self
            .locals
            .iter()
            .rposition(|l| l.depth <= self.current_scope_depth)
            .map_or(0, |i| i + 1);
        self.patch_tdz(keep);
        while self
            .locals
            .last()
            .is_some_and(|l| l.depth > self.current_scope_depth)
        {
            self.locals.pop();
        }
    }

    /// A fresh slot with no name, for compiler-internal values.
    pub(crate) fn alloc_slot(&mut self) -> CompileResult<u8> {
        if self.slot_count >= 256 {
            return Err(CompileError::TooManyLocals);
        }
        let slot = self.slot_count as u8;
        self.slot_count += 1;
        Ok(slot)
    }

    /// The function's scratch slot, for values held only between two adjacent instructions.
    pub(crate) fn temp(&mut self) -> CompileResult<u8> {
        match self.temp_slot {
            Some(slot) => Ok(slot),
            None => {
                let slot = self.alloc_slot()?;
                self.temp_slot = Some(slot);
                Ok(slot)
            }
        }
    }

    /// Declare a name in the current block and return its new slot. Emitted where the
    /// scope starts: a captured name gets a fresh box each time this point runs.
    pub(crate) fn declare_variable(&mut self, name: String) -> CompileResult<u8> {
        let slot = self.alloc_slot()?;
        if self.add_local(name, slot) {
            self.emit_op(Operation::NewBox(slot));
        }
        Ok(slot)
    }

    /// Add a name in the current block; returns whether it's captured (boxed).
    pub(crate) fn add_local(&mut self, name: String, slot: u8) -> bool {
        let captured = self.captured_names.contains(&name);
        self.locals.push(Local {
            name,
            depth: self.current_scope_depth,
            slot,
            captured,
            read_only: false,
            private: None,
            tdz: None,
        });
        captured
    }

    pub(crate) fn local_by_slot(&self, slot: u8) -> &Local {
        self.locals
            .iter()
            .rev()
            .find(|l| l.slot == slot)
            .expect("slot belongs to a declared local")
    }

    /// Push a declared local's value.
    pub(crate) fn emit_load_slot(&mut self, slot: u8) {
        let op = if self.local_by_slot(slot).captured {
            Operation::GetBox(slot)
        } else {
            Operation::GetLocal(slot)
        };
        self.emit_op(op);
    }

    /// Store the top of the stack into a declared local's slot, leaving it on the stack.
    pub(crate) fn emit_store_slot(&mut self, slot: u8) {
        let op = if self.local_by_slot(slot).captured {
            Operation::SetBox(slot)
        } else {
            Operation::SetLocal(slot)
        };
        self.emit_op(op);
    }

    /// Declare a name unless the current block already has it (`var` redeclaring a param, etc).
    pub(crate) fn declare_if_absent(&mut self, name: String) -> CompileResult<u8> {
        let existing = self
            .locals
            .iter()
            .rev()
            .take_while(|l| l.depth == self.current_scope_depth)
            .find(|l| l.name == name);
        match existing {
            Some(local) => Ok(local.slot),
            None => self.declare_variable(name),
        }
    }

    pub(crate) fn resolve_local(&self, name: &str) -> Option<VarRef> {
        if let Some(local) = self.locals.iter().rev().find(|l| l.name == name) {
            return Some(if local.captured {
                VarRef::Box(local.slot)
            } else {
                VarRef::Local(local.slot)
            });
        }
        self.resolve_upvalue(name).map(VarRef::Upvalue)
    }

    /// Find `name` in an enclosing function and return this function's capture index for it,
    /// adding captures along the way (clox-style).
    pub(crate) fn resolve_upvalue(&self, name: &str) -> Option<u8> {
        let enclosing = self.enclosing?;
        let capture = match enclosing.locals.iter().rev().find(|l| l.name == name) {
            Some(local) => {
                debug_assert!(local.captured, "`{name}` is used by a nested function");
                (true, local.slot)
            }
            None => (false, enclosing.resolve_upvalue(name)?),
        };
        let mut upvalues = self.upvalues.borrow_mut();
        if let Some(index) = upvalues.iter().position(|u| *u == capture) {
            return Some(index as u8);
        }
        upvalues.push(capture);
        Some((upvalues.len() - 1) as u8)
    }

    /// Push the value of a variable. Undeclared names are an error, except the host's
    /// `window` / `globalThis`.
    pub(crate) fn emit_get_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        let tdz = self.tdz_use(name, ident.span.lo);
        if tdz == TdzUse::Throw {
            self.emit_op(Operation::ThrowTdz);
            return Ok(());
        }
        match self.resolve_local(name) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::GetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::GetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::GetUpvalue(index)),
            None => match (HostGlobal::from_name(name), name) {
                (Some(host), _) => self.emit_op(Operation::HostGlobal(host as u8)),
                // read-only global constants compile to plain values
                (None, "undefined") => self.emit_op(Operation::Undefined),
                (None, "NaN") => self.emit_op(Operation::LoadFloat64(f64::NAN)),
                (None, "Infinity") => self.emit_op(Operation::LoadFloat64(f64::INFINITY)),
                (None, "arguments") if self.has_arguments() => self.emit_op(Operation::Arguments),
                (None, _) => {
                    return unsupported(format!("undeclared variable `{name}`"), ident.span)
                }
            },
        }
        if tdz == TdzUse::Check {
            self.emit_op(Operation::CheckTdz);
        }
        Ok(())
    }

    /// Whether `arguments` exists here: inside a non-arrow function, or an arrow within one.
    pub(crate) fn has_arguments(&self) -> bool {
        match self.enclosing {
            None => false,
            Some(enclosing) => !self.is_arrow || enclosing.has_arguments(),
        }
    }

    /// Whether `name` resolves to a read-only binding (`const`, a function expression's name).
    pub(crate) fn is_read_only(&self, name: &str) -> bool {
        match self.locals.iter().rev().find(|l| l.name == name) {
            Some(local) => local.read_only,
            None => self.enclosing.is_some_and(|e| e.is_read_only(name)),
        }
    }

    /// Store the top of the stack into a declared variable, leaving it on the stack.
    /// Assigning to a `const` throws a `TypeError` when it runs (after the value is computed).
    pub(crate) fn emit_set_var(&mut self, ident: &Ident) -> CompileResult {
        // writing before the declaration throws too (after the value is computed)
        match self.tdz_use(&ident.sym, ident.span.lo) {
            TdzUse::Throw => {
                self.emit_op(Operation::ThrowTdz);
                return Ok(());
            }
            TdzUse::Check => {
                self.emit_get_var(ident)?;
                self.emit_op(Operation::Pop);
            }
            TdzUse::Free => {}
        }
        if self.is_read_only(&ident.sym) {
            self.emit_op(Operation::Pop);
            self.emit_op(Operation::HostGlobal(HostGlobal::GlobalThis as u8));
            self.emit_op(load_str("TypeError"));
            self.emit_op(Operation::GetProperty);
            self.emit_op(load_str("Assignment to constant variable."));
            self.emit_op(Operation::UInt64(1));
            self.emit_op(Operation::New);
            self.emit_op(Operation::Throw);
            return Ok(());
        }
        self.emit_init_var(ident)
    }

    /// Like `emit_set_var`, but also for initialising a `const` in its declaration.
    pub(crate) fn emit_init_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        match self.resolve_local(name) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::SetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::SetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::SetUpvalue(index)),
            None if HostGlobal::from_name(name).is_some() => {
                return unsupported(format!("assignment to `{name}`"), ident.span)
            }
            None => {
                return unsupported(
                    format!("assignment to undeclared variable `{name}`"),
                    ident.span,
                )
            }
        }
        Ok(())
    }

    pub(crate) fn compile_var_decl(&mut self, var_decl: &VarDecl) -> CompileResult {
        for decl in var_decl.decls.iter() {
            self.compile_var_declarator(var_decl.kind, decl)?;
        }
        Ok(())
    }

    /// The name was already declared when hoisted; this only assigns it.
    pub(crate) fn compile_var_declarator(
        &mut self,
        kind: VarDeclKind,
        var_declarator: &VarDeclarator,
    ) -> CompileResult {
        match (&var_declarator.init, &var_declarator.name) {
            (Some(init), Pat::Ident(ident)) => self.compile_expr_named(init, &ident.id.sym)?,
            (Some(init), _) => self.compile_expr(init)?,
            // `var x;` leaves an existing value alone
            (None, _) if kind == VarDeclKind::Var => return Ok(()),
            (None, _) => self.emit_op(Operation::Undefined),
        }
        self.compile_pattern(&var_declarator.name, true)
    }
}
