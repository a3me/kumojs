//! Compiling functions, arrows and closures.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // functions
    // ---------------------------------------------------------------------------------

    pub(crate) fn compile_fn_decl(&mut self, fn_decl: &FnDecl, internal: bool) -> CompileResult {
        self.hoisting_fn = true;
        let compiled =
            self.compile_function(Some(&fn_decl.ident), &fn_decl.function, false, internal);
        self.hoisting_fn = false;
        compiled?;
        self.emit_set_var(&fn_decl.ident)?;
        self.emit_op(Operation::Pop);
        Ok(())
    }

    /// Push a new function. `self_binding` makes `name` refer to the function inside its
    /// own body (named function expressions). `internal` functions get no host wrapper.
    pub(crate) fn compile_function(
        &mut self,
        name: Option<&Ident>,
        function: &Function,
        self_binding: bool,
        internal: bool,
    ) -> CompileResult {
        let self_name = name.filter(|_| self_binding);
        let name = name.map(|n| &*n.sym);
        self.compile_fn_value(name, self_name, function, internal, FnKind::Normal)
    }

    /// Push a function made from a `Function` node (declarations, expressions, methods).
    pub(crate) fn compile_fn_value(
        &mut self,
        name: Option<&str>,
        self_name: Option<&Ident>,
        function: &Function,
        internal: bool,
        kind: FnKind,
    ) -> CompileResult {
        let params: Vec<&Pat> = function.params.iter().map(|p| &p.pat).collect();
        let body = match &function.body {
            Some(body) => FnBody::Block(&body.stmts),
            None => FnBody::Block(&[]),
        };
        if function.is_generator {
            return unsupported("generator", function.span);
        }
        let is_async = function.is_async;
        // a function value exists from here (declarations are hoisted: they set no anchor)
        if !self.hoisting_fn {
            self.next_anchor = Some(function.span.lo);
        }
        self.compile_closure(name, self_name, &params, &body, internal, kind, is_async)
    }

    pub(crate) fn compile_arrow(
        &mut self,
        arrow: &ArrowExpr,
        name: Option<&str>,
        internal: bool,
    ) -> CompileResult {
        let params: Vec<&Pat> = arrow.params.iter().collect();
        let body = match &*arrow.body {
            ArrowFunctionBody::FunctionBody(block) => FnBody::Block(&block.stmts),
            ArrowFunctionBody::Expr(expr) => FnBody::Expr(expr),
        };
        let is_async = arrow.is_async;
        self.next_anchor = Some(arrow.span.lo);
        self.compile_closure(
            name,
            None,
            &params,
            &body,
            internal,
            FnKind::Arrow,
            is_async,
        )
    }

    /// Compile a function body in its own compiler and push the closure.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_closure(
        &mut self,
        name: Option<&str>,
        self_name: Option<&Ident>,
        params: &[&Pat],
        body: &FnBody,
        internal: bool,
        kind: FnKind,
        is_async: bool,
    ) -> CompileResult {
        let created_at = self
            .next_anchor
            .take()
            .or(self.class_anchor)
            .unwrap_or(BytePos(0));
        let mut c = Compiler::new_enclosing(self);
        c.is_arrow = kind == FnKind::Arrow;
        c.is_async = is_async;
        c.created_at = created_at;
        let body_stmts: Vec<&Stmt> = match body {
            FnBody::Block(stmts) => stmts.iter().collect(),
            _ => Vec::new(),
        };
        c.captured_names = captured_names(params, body, &body_stmts);
        // a base class constructor first sets up the instance's private methods and fields
        // (before its parameters); a derived one does it when `super(...)` returns
        if kind == FnKind::BaseClass {
            c.emit_op(Operation::InitInstance);
            c.emit_op(Operation::Pop);
        }

        // parameters are the first slots; the VM fills them from the call's arguments. A
        // pattern parameter gets a hidden slot that is destructured once all names exist.
        let mut length = None;
        let mut inits = Vec::new();
        let mut rest = None;
        for (i, pat) in params.iter().enumerate() {
            let (target, default) = match pat {
                Pat::Assign(assign) => (&*assign.left, Some(&*assign.right)),
                Pat::Rest(rest_pat) => {
                    rest = Some((&*rest_pat.arg, i as u8));
                    length.get_or_insert(i);
                    break; // the parser only allows it last
                }
                pat => (*pat, None),
            };
            if default.is_some() {
                length.get_or_insert(i);
            }
            let slot = c.alloc_slot()?;
            match target {
                Pat::Ident(ident) => {
                    if c.add_local(ident.id.sym.to_string(), slot) {
                        c.emit_op(Operation::BoxLocal(slot));
                    }
                    if let Some(default) = default {
                        inits.push((slot, Some(default), None, Some(&*ident.id.sym)));
                    }
                }
                pat => inits.push((slot, default, Some(pat), None)),
            }
        }
        let param_count = c.slot_count as u8;
        // names bound by patterns (and the rest parameter)
        let pattern_params = inits.iter().filter_map(|(_, _, pat, _)| *pat);
        for pat in pattern_params.chain(rest.map(|(pat, _)| pat)) {
            for ident in pat_idents(pat) {
                c.declare_variable(ident.sym.to_string())?;
            }
        }
        // defaults and patterns run in parameter order; a default is used when its
        // argument is missing or `undefined`
        for (slot, default, pat, name) in inits {
            match pat {
                None => c.emit_load_slot(slot),
                Some(_) => c.emit_op(Operation::GetLocal(slot)),
            }
            if let Some(default) = default {
                c.emit_default(default, name)?;
            }
            match pat {
                None => {
                    c.emit_store_slot(slot);
                    c.emit_op(Operation::Pop);
                }
                Some(pat) => c.compile_pattern(pat, true)?,
            }
        }
        if let Some((pat, start)) = rest {
            c.emit_op(Operation::RestArgs(start));
            c.compile_pattern(pat, true)?;
        }

        if let Some(name) = self_name {
            // a parameter with the same name wins
            if !c.locals.iter().any(|l| l.name == *name.sym) {
                let slot = c.declare_variable(name.sym.to_string())?;
                c.locals.last_mut().unwrap().read_only = true;
                c.emit_op(Operation::Callee);
                c.emit_store_slot(slot);
                c.emit_op(Operation::Pop);
            }
        }

        match body {
            FnBody::Block(_) => {
                c.compile_body(&body_stmts, true, false)?;
                // implicit `return undefined`
                c.emit_op(Operation::Undefined);
            }
            FnBody::Expr(expr) => {
                c.compile_expr(expr)?;
            }
            FnBody::Field(expr, Some(name)) => c.compile_expr_named(expr, name)?,
            FnBody::Field(expr, None) => c.compile_expr(expr)?,
            FnBody::DefaultDerived => {
                c.emit_op(Operation::RestArgs(0));
                c.emit_op(Operation::SpreadArgs);
                c.emit_op(Operation::SuperCall);
                c.emit_op(Operation::SuperCallFinish);
                c.emit_op(Operation::Pop);
                c.emit_op(Operation::Undefined);
            }
            FnBody::InstanceFields(fields) => {
                for (i, field) in fields.iter().enumerate() {
                    match (field.value, &field.name) {
                        (Some(value), Some(name)) => c.compile_expr_named(value, name)?,
                        (Some(value), None) => c.compile_expr(value)?,
                        (None, _) => c.emit_op(Operation::Undefined),
                    }
                    let name_it = field.name.is_none() && field.value.is_some_and(is_anonymous_fn);
                    c.emit_op(Operation::InitField(i as u32, name_it as u8));
                }
                // the instance is what `super(...)` / the constructor's set-up gives back
                c.emit_op(Operation::This);
            }
        }
        c.emit_op(Operation::Return);
        c.patch_tdz(0);

        let captures = c.upvalues.into_inner();
        // an internal function's name and length can't be observed, so leave the name out
        let name = match name {
            Some(n) if !internal => n.to_string(),
            _ => String::new(),
        };
        self.emit_op(Operation::Function(FnInfo {
            length: length.unwrap_or(params.len()) as u8,
            params: param_count,
            kind,
            is_async,
            internal,
            name,
            captures,
            code: c.bytecode,
        }));
        Ok(())
    }

    pub(crate) fn compile_call(&mut self, expr: &CallExpr) -> CompileResult {
        let callee = match &expr.callee {
            Callee::Expr(callee_expr) => callee_expr,
            Callee::Super(_) => {
                self.compile_args(&expr.args)?;
                self.emit_op(Operation::SuperCall);
                self.emit_op(Operation::SuperCallFinish);
                return Ok(());
            }
            Callee::Import(i) => return unsupported("dynamic import", i.span),
        };

        // `obj.f()` (and `(obj.f)()`) call f with obj as `this`
        let mut unwrapped = &**callee;
        while let Expr::Paren(paren) = unwrapped {
            unwrapped = &paren.expr;
        }
        let is_method = if let Expr::Fn(fn_expr) = unwrapped {
            // IIFE `(function () {...})()`: internal unless its own name escapes in its body
            let internal = match &fn_expr.ident {
                Some(ident) => !fn_escapes_in_function(&ident.sym, &fn_expr.function),
                None => true,
            };
            self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, internal)?;
            false
        } else if let Expr::Arrow(arrow) = unwrapped {
            // `(() => {...})()` can't refer to itself, so it never escapes
            self.compile_arrow(arrow, None, true)?;
            false
        } else if let Expr::SuperProp(super_prop) = unwrapped {
            // `super.m()` calls the parent's m with this `this`: [this, m]
            self.emit_op(Operation::This);
            self.compile_super_prop(&super_prop.prop)?;
            self.emit_op(Operation::SuperGet);
            true
        } else if let Expr::OptChain(chain @ OptChainExpr { base, .. }) = unwrapped {
            // `(a?.b)()` keeps `this`: [obj, f], or [undefined, undefined] if short-circuited
            if !matches!(&**base, OptChainBase::Member(_)) {
                self.compile_expr(unwrapped)?;
                false
            } else {
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, true)?;
                if !nullish.is_empty() {
                    let end = self.emit_jump(Operation::Jump(0));
                    for jump in nullish {
                        self.patch_jump(jump);
                    }
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Undefined);
                    self.emit_op(Operation::Undefined);
                    self.patch_jump(end);
                }
                true
            }
        } else if let Expr::Member(member_expr) = unwrapped {
            // stack: [obj, f]
            self.compile_expr(&member_expr.obj)?;
            self.emit_op(Operation::Dup);
            let access = self.compile_member_prop(&member_expr.prop)?;
            self.emit_member_get(access);
            true
        } else {
            self.compile_expr(callee)?;
            false
        };

        self.compile_args(&expr.args)?;
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
        Ok(())
    }

    /// Push each argument, then the argument count.
    pub(crate) fn compile_args(&mut self, args: &[ExprOrSpread]) -> CompileResult {
        if args.iter().any(|a| a.spread.is_some()) {
            let elems: Vec<Option<ExprOrSpread>> = args.iter().cloned().map(Some).collect();
            self.compile_spread_array(&elems)?;
            self.emit_op(Operation::SpreadArgs);
            return Ok(());
        }
        for arg in args {
            self.compile_expr(&arg.expr)?;
        }
        self.emit_op(Operation::UInt64(args.len() as u64));
        Ok(())
    }

    /// Build an array element by element, for literals and argument lists containing `...x`.
    pub(crate) fn compile_spread_array(&mut self, elems: &[Option<ExprOrSpread>]) -> CompileResult {
        self.emit_op(Operation::Array(0));
        for elem in elems {
            match elem {
                Some(ExprOrSpread {
                    spread: Some(_),
                    expr,
                }) => {
                    self.compile_expr(expr)?;
                    self.emit_op(Operation::ArraySpread);
                }
                Some(ExprOrSpread { spread: None, expr }) => {
                    self.compile_expr(expr)?;
                    self.emit_op(Operation::ArrayAppend);
                }
                None => {
                    self.emit_op(Operation::Hole);
                    self.emit_op(Operation::ArrayAppend);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn compile_new(&mut self, expr: &NewExpr) -> CompileResult {
        self.compile_expr(&expr.callee)?;
        self.compile_args(expr.args.as_deref().unwrap_or_default())?;
        self.emit_op(Operation::New);
        Ok(())
    }
}
