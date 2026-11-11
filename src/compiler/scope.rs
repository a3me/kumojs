//! Scopes and declarations: locals, slots, upvalues, hoisting and the temporal dead zone.

use super::prelude::*;

impl FnCompiler<'_> {
    pub(super) fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    /// Leave a block: its names go out of scope (their slots are not reused).
    pub(super) fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
        let keep = self
            .locals
            .iter()
            .rposition(|l| l.depth <= self.current_scope_depth)
            .map_or(0, |i| i + 1);
        self.patch_tdz(keep);
        self.locals.truncate(keep);
    }

    // ---- slots and locals

    /// A fresh slot with no name, for compiler-internal values.
    pub(super) fn alloc_slot(&mut self) -> CompileResult<u8> {
        if self.slot_count >= 256 {
            return Err(CompileError::TooManyLocals);
        }
        let slot = self.slot_count as u8;
        self.slot_count += 1;
        Ok(slot)
    }

    /// The function's scratch slot, for values held only between two adjacent instructions.
    pub(super) fn temp(&mut self) -> CompileResult<u8> {
        if let Some(slot) = self.temp_slot {
            return Ok(slot);
        }
        let slot = self.alloc_slot()?;
        self.temp_slot = Some(slot);
        Ok(slot)
    }

    /// Declare a name in the current block and return its new slot. Emitted where the
    /// scope starts: a captured name gets a fresh box each time this point runs.
    pub(super) fn declare_variable(&mut self, name: String) -> CompileResult<u8> {
        let slot = self.alloc_slot()?;
        if self.add_local(name, slot) {
            self.emit_op(Operation::NewBox(slot));
        }
        Ok(slot)
    }

    /// Declare a name unless the current block already has it (`var` redeclaring a param, etc).
    pub(super) fn declare_if_absent(&mut self, name: String) -> CompileResult<u8> {
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

    /// Add a name in the current block; returns whether it's captured (boxed).
    pub(super) fn add_local(&mut self, name: String, slot: u8) -> bool {
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

    /// The local declared last.
    pub(super) fn last_local(&mut self) -> &mut Local {
        self.locals.last_mut().expect("a local was just declared")
    }

    fn find_local(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|l| l.name == name)
    }

    /// How a declared local's slot is read and written.
    fn slot_ref(&self, slot: u8) -> VarRef {
        let local = self
            .locals
            .iter()
            .rev()
            .find(|l| l.slot == slot)
            .expect("slot belongs to a declared local");
        if local.captured {
            VarRef::Box(slot)
        } else {
            VarRef::Local(slot)
        }
    }

    /// Push a declared local's value.
    pub(super) fn emit_load_slot(&mut self, slot: u8) {
        self.emit_load_ref(self.slot_ref(slot));
    }

    /// Store the top of the stack into a declared local's slot, leaving it on the stack.
    pub(super) fn emit_store_slot(&mut self, slot: u8) {
        self.emit_store_ref(self.slot_ref(slot));
    }

    // ---- resolving names

    pub(super) fn resolve_local(&self, name: &str) -> Option<VarRef> {
        if let Some(local) = self.find_local(name) {
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
    fn resolve_upvalue(&self, name: &str) -> Option<u8> {
        let enclosing = self.enclosing?;
        let capture = match enclosing.find_local(name) {
            Some(local) => {
                debug_assert!(local.captured, "`{name}` is used by a nested function");
                Capture::Local(local.slot)
            }
            None => Capture::Upvalue(enclosing.resolve_upvalue(name)?),
        };
        let mut upvalues = self.upvalues.borrow_mut();
        if let Some(index) = upvalues.iter().position(|u| *u == capture) {
            return Some(index as u8);
        }
        upvalues.push(capture);
        Some((upvalues.len() - 1) as u8)
    }

    /// Whether `arguments` exists here: inside a non-arrow function, or an arrow within one.
    pub(super) fn has_arguments(&self) -> bool {
        match self.enclosing {
            None => false,
            Some(enclosing) => !self.is_arrow || enclosing.has_arguments(),
        }
    }

    /// Whether `name` resolves to a read-only binding (`const`, a function expression's name).
    fn is_read_only(&self, name: &str) -> bool {
        match self.find_local(name) {
            Some(local) => local.read_only,
            None => self.enclosing.is_some_and(|e| e.is_read_only(name)),
        }
    }

    /// The kind of the innermost private name called `key` (`#x`) in scope.
    pub(super) fn private_kind(&self, key: &str) -> Option<PrivateKind> {
        match self.find_local(key) {
            Some(local) => local.private,
            None => self.enclosing?.private_kind(key),
        }
    }

    // ---- reading and writing variables

    /// Push the value of a variable. Undeclared names are an error, except the host's
    /// `window` / `globalThis`.
    pub(super) fn emit_get_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        let tdz = self.tdz_use(name, ident.span.lo);
        if tdz == TdzUse::Throw {
            self.emit_op(Operation::ThrowTdz);
            return Ok(());
        }
        match self.resolve_local(name) {
            Some(var) => self.emit_load_ref(var),
            None => match (HostGlobal::from_name(name), name) {
                (Some(host), _) => self.emit_op(Operation::HostGlobal(host, false)),
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

    /// Store the top of the stack into a declared variable, leaving it on the stack.
    /// Assigning to a `const` throws a `TypeError` when it runs (after the value is computed).
    pub(super) fn emit_set_var(&mut self, ident: &Ident) -> CompileResult {
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
            self.emit_op(Operation::HostGlobal(HostGlobal::GlobalThis, false));
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
    pub(super) fn emit_init_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        match self.resolve_local(name) {
            Some(var) => self.emit_store_ref(var),
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

    /// `emit_init_var` or `emit_set_var`.
    pub(super) fn emit_bind_var(&mut self, ident: &Ident, bind: Bind) -> CompileResult {
        match bind {
            Bind::Init => self.emit_init_var(ident),
            Bind::Assign => self.emit_set_var(ident),
        }
    }

    // ---- hoisting

    /// Declarations are hoisted before any code runs: `var`s (function bodies only, found
    /// anywhere in the body), then `let`/`const` and function names of this block, then
    /// function values, so code can refer to anything in scope regardless of order.
    pub(super) fn hoist_declarations(&mut self, stmts: &[&Stmt], kind: BodyKind) -> CompileResult {
        let function_top = kind != BodyKind::Block;
        if function_top {
            let mut names = Vec::new();
            let mut collector = VarCollector { names: &mut names };
            for stmt in stmts {
                stmt.visit_with(&mut collector);
            }
            for name in names {
                self.declare_if_absent(name)?;
            }
        }

        self.check_redeclarations(stmts, function_top)?;
        for stmt in stmts {
            match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => {
                    self.declare_lexical(var_decl, None)?;
                }
                Stmt::Decl(Decl::Fn(fn_decl)) => {
                    self.declare_if_absent(fn_decl.ident.sym.to_string())?;
                }
                Stmt::Decl(Decl::Class(class_decl)) => {
                    let name = class_decl.ident.sym.to_string();
                    self.declare_with_tdz(name, class_decl.class.span.hi)?;
                }
                _ => {}
            }
        }

        for stmt in stmts {
            if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
                // `stmts` is everything that can see this name, so it's enough to check
                let internal = !fn_escapes(&fn_decl.ident.sym, stmts);
                self.compile_fn_decl(fn_decl, internal)?;
            }
        }
        Ok(())
    }

    /// Early errors for names declared twice in one block: a `let` / `const` / `class` may
    /// share its name with nothing else there (including parameters and `var`s at a
    /// function's top level), and block-level functions can't repeat either.
    fn check_redeclarations(&self, stmts: &[&Stmt], function_top: bool) -> CompileResult {
        let mut seen: Vec<(&str, bool)> = Vec::new(); // (name, lexical)
        for stmt in stmts {
            let names: Vec<(&Ident, bool)> = match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => var_decl
                    .decls
                    .iter()
                    .flat_map(|d| pat_idents(&d.name))
                    .map(|i| (i, true))
                    .collect(),
                Stmt::Decl(Decl::Class(class_decl)) => vec![(&class_decl.ident, true)],
                Stmt::Decl(Decl::Fn(fn_decl)) => vec![(&fn_decl.ident, false)],
                _ => continue,
            };
            for (ident, lexical) in names {
                let name = &*ident.sym;
                let clashes_in_block = seen.iter().any(|&(n, other_lexical)| {
                    n == name && (lexical || other_lexical || !function_top)
                });
                let clashes_with_outer = lexical
                    && self
                        .locals
                        .iter()
                        .any(|l| l.name == name && l.depth == self.current_scope_depth);
                if clashes_in_block || clashes_with_outer {
                    return syntax_error(format!("`{name}` has already been declared"), ident.span);
                }
                seen.push((name, lexical));
            }
        }
        Ok(())
    }

    /// Declare a `let` / `const`'s names. `end` overrides where each is initialised
    /// (a `for-in/of` head: after the iterated expression).
    pub(super) fn declare_lexical(
        &mut self,
        var_decl: &VarDecl,
        end: Option<BytePos>,
    ) -> CompileResult {
        for decl in &var_decl.decls {
            for ident in pat_idents(&decl.name) {
                self.declare_with_tdz(ident.sym.to_string(), end.unwrap_or(decl.span.hi))?;
                if var_decl.kind == VarDeclKind::Const {
                    self.last_local().read_only = true;
                }
            }
        }
        Ok(())
    }

    // ---- the temporal dead zone

    /// Declare a `let` / `const` / `class` name whose temporal dead zone ends at `decl_end`.
    pub(super) fn declare_with_tdz(&mut self, name: String, decl_end: BytePos) -> CompileResult<u8> {
        let new_box_at = self.bytecode.len();
        let slot = self.declare_variable(name)?;
        let box_at = self.last_local().captured.then_some(new_box_at);
        let always = self.in_switch;
        if always {
            // uninitialised from the start of the switch
            match box_at {
                Some(at) => self.bytecode[at] = self.op_byte(&Operation::NewBoxTdz(0)),
                None => self.emit_op(Operation::Tdz(slot)),
            }
        }
        self.last_local().tdz = Some(Tdz {
            decl_end,
            always,
            box_at,
            needed: Cell::new(false),
        });
        Ok(slot)
    }

    /// Bindings that a closure checks start uninitialised: turn their `NewBox` into
    /// `NewBoxTdz`. Done as each leaves scope (or at the end of the function).
    pub(super) fn patch_tdz(&mut self, from: usize) {
        let new_box_tdz = self.op_byte(&Operation::NewBoxTdz(0));
        for local in &self.locals[from..] {
            if let Some(Tdz {
                box_at: Some(at),
                needed,
                always: false,
                ..
            }) = &local.tdz
            {
                if needed.get() {
                    self.bytecode[*at] = new_box_tdz;
                }
            }
        }
    }

    /// How a use of `name` at `pos` deals with the temporal dead zone.
    fn tdz_use(&self, name: &str, pos: BytePos) -> TdzUse {
        let check = |tdz: &Tdz, at: BytePos, same_function: bool| {
            if tdz.always {
                TdzUse::Check
            } else if at >= tdz.decl_end {
                TdzUse::Free
            } else if same_function {
                TdzUse::Throw
            } else {
                tdz.needed.set(true);
                TdzUse::Check
            }
        };
        if let Some(local) = self.find_local(name) {
            return local
                .tdz
                .as_ref()
                .map_or(TdzUse::Free, |t| check(t, pos, true));
        }
        // a closure: what matters is where it (or the closure containing it) was made
        let mut anchor = self.created_at;
        let mut compiler = self.enclosing;
        while let Some(c) = compiler {
            if let Some(local) = c.find_local(name) {
                return local
                    .tdz
                    .as_ref()
                    .map_or(TdzUse::Free, |t| check(t, anchor, false));
            }
            anchor = c.created_at;
            compiler = c.enclosing;
        }
        TdzUse::Free
    }
}
