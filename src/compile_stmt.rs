//! Compiling statements.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // statements
    // ---------------------------------------------------------------------------------

    /// Compile a list of statements forming a function body or block.
    /// `leave_last` keeps the last expression statement's value on the stack (module body).
    pub(crate) fn compile_body(
        &mut self,
        stmts: &[&Stmt],
        hoist_vars: bool,
        leave_last: bool,
    ) -> CompileResult {
        self.hoist_declarations(stmts, hoist_vars)?;
        let last = stmts.len().saturating_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            self.compile_stmt(stmt, leave_last && i == last)?;
        }
        Ok(())
    }

    /// Declarations are hoisted before any code runs: `var`s (function bodies only, found
    /// anywhere in the body), then `let`/`const` and function names of this block, then
    /// function values, so code can refer to anything in scope regardless of order.
    pub(crate) fn hoist_declarations(
        &mut self,
        stmts: &[&Stmt],
        hoist_vars: bool,
    ) -> CompileResult {
        if hoist_vars {
            let mut names = Vec::new();
            let mut collector = VarCollector { names: &mut names };
            for stmt in stmts {
                stmt.visit_with(&mut collector);
            }
            for name in names {
                self.declare_if_absent(name)?;
            }
        }

        self.check_redeclarations(stmts, hoist_vars)?;
        for stmt in stmts {
            match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => {
                    self.declare_lexical(var_decl, None)?;
                }
                Stmt::Decl(Decl::Fn(fn_decl)) => {
                    self.declare_if_absent(fn_decl.ident.sym.to_string())?;
                }
                Stmt::Decl(Decl::Class(class_decl)) => {
                    self.declare_variable(class_decl.ident.sym.to_string())?;
                    self.mark_tdz(class_decl.class.span.hi);
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
    pub(crate) fn check_redeclarations(
        &self,
        stmts: &[&Stmt],
        function_top: bool,
    ) -> CompileResult {
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
                let clash = seen.iter().any(|&(n, other_lexical)| {
                    n == name && (lexical || other_lexical || !function_top)
                }) || (lexical
                    && self
                        .locals
                        .iter()
                        .any(|l| l.name == name && l.depth == self.current_scope_depth));
                if clash {
                    return Err(CompileError::SyntaxError {
                        what: format!("`{name}` has already been declared"),
                        span: ident.span,
                        location: String::new(),
                    });
                }
                seen.push((name, lexical));
            }
        }
        Ok(())
    }

    /// Declare a `let` / `const`'s names. `end` overrides where each is initialised
    /// (a `for-in/of` head: after the iterated expression).
    pub(crate) fn declare_lexical(
        &mut self,
        var_decl: &VarDecl,
        end: Option<BytePos>,
    ) -> CompileResult {
        for decl in &var_decl.decls {
            for ident in pat_idents(&decl.name) {
                self.declare_variable(ident.sym.to_string())?;
                if var_decl.kind == VarDeclKind::Const {
                    self.locals.last_mut().unwrap().read_only = true;
                }
                self.mark_tdz(end.unwrap_or(decl.span.hi));
            }
        }
        Ok(())
    }

    /// Give the local just declared a temporal dead zone ending at `decl_end`.
    pub(crate) fn mark_tdz(&mut self, decl_end: BytePos) {
        let always = self.in_switch;
        let local = self.locals.last().unwrap();
        // its `NewBox` (if captured) was the last instruction
        let box_at = local.captured.then(|| self.bytecode.len() - 2);
        let slot = local.slot;
        if always {
            // uninitialised from the start of the switch
            match box_at {
                Some(at) => self.bytecode[at] = Operation::NewBoxTdz(0).get_opcode(),
                None => self.emit_op(Operation::Tdz(slot)),
            }
        }
        self.locals.last_mut().unwrap().tdz = Some(Tdz {
            decl_end,
            always,
            box_at,
            needed: Cell::new(false),
        });
    }

    /// Bindings that a closure checks start uninitialised: turn their `NewBox` into
    /// `NewBoxTdz`. Done as each leaves scope (or at the end of the function).
    pub(crate) fn patch_tdz(&mut self, from: usize) {
        let opcode = Operation::NewBoxTdz(0).get_opcode();
        for local in &self.locals[from..] {
            if let Some(Tdz {
                box_at: Some(at),
                needed,
                always: false,
                ..
            }) = &local.tdz
            {
                if needed.get() {
                    self.bytecode[*at] = opcode;
                }
            }
        }
    }

    /// How a use of `name` at `pos` deals with the temporal dead zone.
    pub(crate) fn tdz_use(&self, name: &str, pos: BytePos) -> TdzUse {
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
        if let Some(local) = self.locals.iter().rev().find(|l| l.name == name) {
            return local
                .tdz
                .as_ref()
                .map_or(TdzUse::Free, |t| check(t, pos, true));
        }
        // a closure: what matters is where it (or the closure containing it) was made
        let mut anchor = self.created_at;
        let mut compiler = self.enclosing;
        while let Some(c) = compiler {
            if let Some(local) = c.locals.iter().rev().find(|l| l.name == name) {
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

    pub(crate) fn compile_stmt(&mut self, stmt: &Stmt, is_last_in_module: bool) -> CompileResult {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                // leave the last statement's value on the stack so the test runner can read it
                if is_last_in_module {
                    self.compile_expr(&expr_stmt.expr)?;
                } else {
                    self.compile_discarded(&expr_stmt.expr)?;
                }
            }
            Stmt::Return(return_stmt) => {
                match &return_stmt.arg {
                    Some(arg) => self.compile_expr(arg)?,
                    None => self.emit_op(Operation::Undefined),
                }
                self.emit_unwind_tries(0)?;
                self.emit_op(Operation::Return);
            }
            // compiled when hoisted by `hoist_declarations`
            Stmt::Decl(Decl::Fn(_)) => {}
            Stmt::Decl(Decl::Class(class_decl)) => {
                let ident = &class_decl.ident;
                self.compile_class(Some(ident), &class_decl.class, Some(&ident.sym))?;
                self.emit_init_var(ident)?;
                self.emit_op(Operation::Pop);
            }
            Stmt::Decl(Decl::Var(var_decl)) => self.compile_var_decl(var_decl)?,
            Stmt::Block(block_stmt) => self.compile_block(block_stmt)?,
            Stmt::Empty(_) | Stmt::Debugger(_) => {}
            Stmt::If(if_stmt) => self.compile_if(if_stmt)?,
            Stmt::While(while_stmt) => self.compile_while(while_stmt)?,
            Stmt::DoWhile(do_while) => self.compile_do_while(do_while)?,
            Stmt::For(for_stmt) => self.compile_for(for_stmt)?,
            Stmt::ForIn(for_in) => self.compile_for_in(for_in)?,
            Stmt::ForOf(for_of) => self.compile_for_of(for_of)?,
            Stmt::Labeled(labeled) => self.compile_labeled(labeled)?,
            Stmt::Break(break_stmt) => {
                let label = break_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, false, break_stmt.span)?;
                self.emit_unwind_tries(self.breakables[target].try_depth)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].breaks.push(jump);
            }
            Stmt::Continue(continue_stmt) => {
                let label = continue_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, true, continue_stmt.span)?;
                // a `for-of` keeps its iterator open: its body region is left at the loop
                let loop_ = &self.breakables[target];
                self.emit_unwind_tries(loop_.try_depth + loop_.iter_region as usize)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].continues.push(jump);
            }
            Stmt::Switch(switch_stmt) => self.compile_switch(switch_stmt)?,
            Stmt::Throw(throw_stmt) => {
                self.compile_expr(&throw_stmt.arg)?;
                self.emit_op(Operation::Throw);
            }
            Stmt::Try(try_stmt) => self.compile_try(try_stmt)?,
            _ => return unsupported(stmt_kind(stmt), stmt.span()),
        }
        Ok(())
    }

    pub(crate) fn compile_block(&mut self, block: &BlockStmt) -> CompileResult {
        let stmts: Vec<&Stmt> = block.stmts.iter().collect();
        self.enter_scope();
        self.compile_body(&stmts, false, false)?;
        self.exit_scope();
        Ok(())
    }

    pub(crate) fn compile_if(&mut self, stmt: &IfStmt) -> CompileResult {
        self.compile_expr(&stmt.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.cons, false)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        if let Some(alt) = &stmt.alt {
            self.compile_stmt(alt, false)?;
        }
        self.patch_jump(end_jump);
        Ok(())
    }

    pub(crate) fn compile_while(&mut self, stmt: &WhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    pub(crate) fn compile_do_while(&mut self, stmt: &DoWhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    pub(crate) fn compile_for(&mut self, stmt: &ForStmt) -> CompileResult {
        // `let` in the initializer is scoped to the loop
        self.enter_scope();
        // captured `let`/`const` loop variables get a fresh binding for each iteration
        let mut per_iteration = Vec::new();
        match &stmt.init {
            Some(VarDeclOrExpr::VarDecl(var_decl)) => {
                if var_decl.kind != VarDeclKind::Var {
                    let first = self.locals.len();
                    self.declare_lexical(var_decl, None)?;
                    per_iteration = self.locals[first..]
                        .iter()
                        .filter(|l| l.captured)
                        .map(|l| l.slot)
                        .collect();
                }
                self.compile_var_decl(var_decl)?;
                // closures made in the initializer keep the initial binding
                for &slot in &per_iteration {
                    self.emit_op(Operation::RenewBox(slot));
                }
            }
            Some(VarDeclOrExpr::Expr(expr)) => {
                self.compile_expr(expr)?;
                self.emit_op(Operation::Pop);
            }
            None => {}
        }

        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        let exit = match &stmt.test {
            Some(test) => {
                self.compile_expr(test)?;
                let exit = self.emit_jump(Operation::JumpIfFalse(0));
                self.emit_op(Operation::Pop);
                Some(exit)
            }
            None => None,
        };
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        // the next iteration starts with a copy, so closures from this one keep their value
        for &slot in &per_iteration {
            self.emit_op(Operation::RenewBox(slot));
        }
        if let Some(update) = &stmt.update {
            self.compile_discarded(update)?;
        }
        self.emit_loop(start);
        if let Some(exit) = exit {
            self.patch_jump(exit);
            self.emit_op(Operation::Pop);
        }
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    pub(crate) fn compile_for_in(&mut self, stmt: &ForInStmt) -> CompileResult {
        self.enter_scope();

        // keys array and index live in hidden slots, so break/continue leave nothing on the stack
        let keys = self.alloc_slot()?;
        let index = self.alloc_slot()?;
        // the head's names exist (uninitialised) while the object expression runs
        let target = self.loop_head_target(&stmt.left, &stmt.right)?;
        self.compile_expr(&stmt.right)?;
        self.emit_op(Operation::ForInKeys);
        self.emit_op(Operation::SetLocal(keys));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::LoadFloat64(0.0));
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);

        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::GetLocal(keys));
        self.emit_op(load_str("length"));
        self.emit_op(Operation::GetProperty);
        self.emit_op(Operation::Lt);
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);

        // target = keys[index]
        self.emit_op(Operation::GetLocal(keys));
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::GetProperty);
        self.bind_loop_head(&stmt.left, target)?;

        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::Increment);
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    /// The pattern a `for-in` / `for-of` head assigns each value to, declaring its names.
    pub(crate) fn loop_head_target<'h>(
        &mut self,
        head: &'h ForHead,
        right: &Expr,
    ) -> CompileResult<&'h Pat> {
        match head {
            ForHead::VarDecl(var_decl) => {
                if var_decl.kind != VarDeclKind::Var {
                    // the iterated expression still sees them uninitialised
                    self.declare_lexical(var_decl, Some(right.span().hi))?;
                }
                match var_decl.decls.as_slice() {
                    [decl] if decl.init.is_none() => Ok(&decl.name),
                    _ => unsupported("for-in/of declaration", var_decl.span),
                }
            }
            ForHead::Pat(pat) => Ok(pat),
            ForHead::UsingDecl(using) => unsupported("using declaration", using.span),
        }
    }

    /// Store the value on top of the stack into a loop head's target. A `let` / `const`
    /// head gets fresh bindings each iteration.
    pub(crate) fn bind_loop_head(&mut self, head: &ForHead, target: &Pat) -> CompileResult {
        if matches!(head, ForHead::VarDecl(d) if d.kind != VarDeclKind::Var) {
            for ident in pat_idents(target) {
                if let Some(VarRef::Box(slot)) = self.resolve_local(&ident.sym) {
                    self.emit_op(Operation::NewBox(slot));
                }
            }
        }
        self.compile_pattern(target, matches!(head, ForHead::VarDecl(_)))
    }

    /// `for (x of iterable)`. Each iteration's binding and body run in a protected region:
    /// leaving it early (`break`, `return`, a throw) closes the iterator.
    ///
    ///   GetIterator -> it; L: IterNext(it, X) TryStart(H) bind body C: TryEnd Loop L;
    ///   H: IterClose(it, throw) rethrow; X:
    pub(crate) fn compile_for_of(&mut self, stmt: &ForOfStmt) -> CompileResult {
        if stmt.is_await {
            return unsupported("for await", stmt.span);
        }
        self.enter_scope();
        // the iterator lives in a hidden slot, so break/continue leave nothing on the stack
        let iterator = self.alloc_slot()?;
        // the iterable is evaluated with the head's names declared (but not initialised)
        let target = self.loop_head_target(&stmt.left, &stmt.right)?;
        self.compile_expr(&stmt.right)?;
        self.emit_op(Operation::GetIterator);
        self.emit_op(Operation::SetLocal(iterator));
        self.emit_op(Operation::Pop);

        self.push_breakable(BreakableKind::Loop);
        self.breakables.last_mut().unwrap().iter_region = true;
        let start = self.bytecode.len();
        let exit = self.emit_jump(Operation::IterNext(iterator, 0));
        let handler = self.emit_jump(Operation::TryStart(0));
        self.push_try_region(None);
        self.try_stack.last_mut().unwrap().close_iterator = Some(iterator);
        self.bind_loop_head(&stmt.left, target)?;
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.try_stack.pop();
        self.emit_op(Operation::TryEnd);
        self.emit_loop(start);

        // thrown from the binding or body: close the iterator (ignoring its errors), rethrow
        self.patch_jump(handler);
        let temp = self.temp()?;
        self.emit_op(Operation::SetLocal(temp));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::IterClose(iterator, 1));
        self.emit_op(Operation::GetLocal(temp));
        self.emit_op(Operation::Throw);

        self.patch_jump(exit);
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    pub(crate) fn compile_labeled(&mut self, stmt: &LabeledStmt) -> CompileResult {
        self.pending_labels.push(stmt.label.sym.to_string());
        match &*stmt.body {
            // these attach the pending labels themselves
            Stmt::While(_)
            | Stmt::DoWhile(_)
            | Stmt::For(_)
            | Stmt::ForIn(_)
            | Stmt::ForOf(_)
            | Stmt::Switch(_)
            | Stmt::Labeled(_) => self.compile_stmt(&stmt.body, false),
            body => {
                self.push_breakable(BreakableKind::Labeled);
                self.compile_stmt(body, false)?;
                self.pop_breakable();
                Ok(())
            }
        }
    }

    pub(crate) fn compile_switch(&mut self, stmt: &SwitchStmt) -> CompileResult {
        // the whole switch body is one block scope
        self.enter_scope();
        let discriminant = self.alloc_slot()?;
        self.compile_expr(&stmt.discriminant)?;
        self.emit_op(Operation::SetLocal(discriminant));
        self.emit_op(Operation::Pop);

        let all_stmts: Vec<&Stmt> = stmt.cases.iter().flat_map(|c| c.cons.iter()).collect();
        self.in_switch = true;
        let hoisted = self.hoist_declarations(&all_stmts, false);
        self.in_switch = false;
        hoisted?;

        self.push_breakable(BreakableKind::Switch);

        // tests, in order, each jumping to its case body
        let mut case_jumps = Vec::with_capacity(stmt.cases.len());
        for case in &stmt.cases {
            match &case.test {
                Some(test) => {
                    self.emit_op(Operation::GetLocal(discriminant));
                    self.compile_expr(test)?;
                    self.emit_op(Operation::EqEqEq);
                    let next = self.emit_jump(Operation::JumpIfFalse(0));
                    self.emit_op(Operation::Pop);
                    case_jumps.push(Some(self.emit_jump(Operation::Jump(0))));
                    self.patch_jump(next);
                    self.emit_op(Operation::Pop);
                }
                None => case_jumps.push(None),
            }
        }
        // no case matched: go to `default`, or past the switch
        let default_jump = self.emit_jump(Operation::Jump(0));
        let mut has_default = false;

        // bodies, in order, falling through
        for (case, jump) in stmt.cases.iter().zip(case_jumps) {
            match jump {
                Some(jump) => self.patch_jump(jump),
                None => {
                    self.patch_jump(default_jump);
                    has_default = true;
                }
            }
            for stmt in &case.cons {
                self.compile_stmt(stmt, false)?;
            }
        }
        if !has_default {
            self.breakables
                .last_mut()
                .unwrap()
                .breaks
                .push(default_jump);
        }

        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    // Layouts (H = catch handler, FT = finally-on-throw, F = finally block):
    //   try/catch:          TryStart(H) B TryEnd Jump END; H: bind C; END:
    //   try/finally:        TryStart(FT) B TryEnd Jump N; FT: save F rethrow; N: F
    //   try/catch/finally:  TryStart(H) B TryEnd Jump N; H: bind TryStart(FT) C TryEnd Jump N;
    //                       FT: save F rethrow; N: F
    pub(crate) fn compile_try(&mut self, stmt: &TryStmt) -> CompileResult {
        let finally = stmt.finalizer.as_ref();

        let handler = self.emit_jump(Operation::TryStart(0));
        self.push_try_region(finally);
        self.compile_block(&stmt.block)?;
        self.try_stack.pop();
        self.emit_op(Operation::TryEnd);
        let mut to_normal_end = vec![self.emit_jump(Operation::Jump(0))];
        self.patch_jump(handler);

        let mut finally_handler = None;
        if let Some(catch) = &stmt.handler {
            // the thrown value is on the stack
            self.enter_scope();
            match &catch.param {
                Some(pat) => {
                    for ident in pat_idents(pat) {
                        self.declare_variable(ident.sym.to_string())?;
                    }
                    self.compile_pattern(pat, true)?;
                }
                None => self.emit_op(Operation::Pop),
            }

            if finally.is_some() {
                finally_handler = Some(self.emit_jump(Operation::TryStart(0)));
                self.push_try_region(finally);
            }
            self.compile_block(&catch.body)?;
            if finally.is_some() {
                self.try_stack.pop();
                self.emit_op(Operation::TryEnd);
                to_normal_end.push(self.emit_jump(Operation::Jump(0)));
            }
            self.exit_scope();
        }

        if let Some(finally) = finally {
            // thrown out of the try (or catch): run finally, then rethrow
            if let Some(jump) = finally_handler {
                self.patch_jump(jump);
            }
            let saved = self.alloc_slot()?;
            self.emit_op(Operation::SetLocal(saved));
            self.emit_op(Operation::Pop);
            self.compile_block(finally)?;
            self.emit_op(Operation::GetLocal(saved));
            self.emit_op(Operation::Throw);

            for jump in to_normal_end {
                self.patch_jump(jump);
            }
            self.compile_block(finally)?;
        } else {
            for jump in to_normal_end {
                self.patch_jump(jump);
            }
        }
        Ok(())
    }

    pub(crate) fn push_try_region(&mut self, finally: Option<&BlockStmt>) {
        self.try_stack.push(TryRegion {
            finally: finally.cloned(),
            close_iterator: None,
            locals_len: self.locals.len(),
            scope_depth: self.current_scope_depth,
        });
    }

    /// Leave every try region above `depth` (innermost first): drop its handler and run
    /// its `finally` inline. The regions stay on `try_stack`, since only this exit path leaves them.
    pub(crate) fn emit_unwind_tries(&mut self, depth: usize) -> CompileResult {
        let mut i = self.try_stack.len();
        while i > depth {
            i -= 1;
            self.emit_op(Operation::TryEnd);
            if let Some(iterator) = self.try_stack[i].close_iterator {
                self.emit_op(Operation::IterClose(iterator, 0));
            }
            let Some(finally) = self.try_stack[i].finally.clone() else {
                continue;
            };

            // compile the finally as if at the `try` statement: outside region i and
            // everything nested in it
            let locals_len = self.try_stack[i].locals_len;
            let scope_depth = self.try_stack[i].scope_depth;
            let inner_tries = self.try_stack.split_off(i);
            let inner_locals = self.locals.split_off(locals_len);
            let inner_depth = std::mem::replace(&mut self.current_scope_depth, scope_depth);
            let first_inner = self
                .breakables
                .iter()
                .position(|b| b.try_depth > i)
                .unwrap_or(self.breakables.len());
            let inner_breakables = self.breakables.split_off(first_inner);

            let result = self.compile_block(&finally);

            self.breakables.extend(inner_breakables);
            self.current_scope_depth = inner_depth;
            self.locals.extend(inner_locals);
            self.try_stack.extend(inner_tries);
            result?;
        }
        Ok(())
    }

    pub(crate) fn push_breakable(&mut self, kind: BreakableKind) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.breakables.push(Breakable {
            kind,
            labels,
            breaks: Vec::new(),
            continues: Vec::new(),
            try_depth: self.try_stack.len(),
            iter_region: false,
        });
    }

    pub(crate) fn patch_continues(&mut self) {
        let continues = std::mem::take(&mut self.breakables.last_mut().unwrap().continues);
        for jump in continues {
            self.patch_jump(jump);
        }
    }

    pub(crate) fn pop_breakable(&mut self) {
        let breakable = self.breakables.pop().unwrap();
        for jump in breakable.breaks {
            self.patch_jump(jump);
        }
    }

    pub(crate) fn find_breakable(
        &self,
        label: Option<&str>,
        is_continue: bool,
        span: Span,
    ) -> CompileResult<usize> {
        let found = self.breakables.iter().rposition(|b| match label {
            Some(label) => b.labels.iter().any(|l| l == label),
            None if is_continue => b.kind == BreakableKind::Loop,
            None => b.kind != BreakableKind::Labeled,
        });
        match found {
            Some(i) if is_continue && self.breakables[i].kind != BreakableKind::Loop => {
                unsupported("continue to a non-loop label", span)
            }
            Some(i) => Ok(i),
            None if is_continue => unsupported("continue outside a loop", span),
            None => unsupported("break outside a loop or switch", span),
        }
    }
}
