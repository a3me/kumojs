//! Compiling functions, arrows and closures.

use super::prelude::*;

impl FnCompiler<'_> {
    /// A hoisted function declaration: create it and store it in its (already declared) name.
    pub(super) fn compile_fn_decl(&mut self, fn_decl: &FnDecl, internal: bool) -> CompileResult {
        let options = FnOptions {
            name: Some(&fn_decl.ident.sym),
            internal,
            ..FnOptions::new(FnKind::Normal)
        };
        self.hoisting_fn = true;
        let compiled = self.compile_fn_value(&fn_decl.function, options);
        self.hoisting_fn = false;
        compiled?;
        self.emit_set_var(&fn_decl.ident)?;
        self.emit_op(Operation::Pop);
        Ok(())
    }

    /// Push a function expression. Its own name (if any) refers to it inside its body.
    pub(super) fn compile_fn_expr(&mut self, fn_expr: &FnExpr, internal: bool) -> CompileResult {
        let options = FnOptions {
            name: fn_expr.ident.as_ref().map(|i| &*i.sym),
            self_name: fn_expr.ident.as_ref(),
            internal,
            ..FnOptions::new(FnKind::Normal)
        };
        self.compile_fn_value(&fn_expr.function, options)
    }

    /// Push a function made from a `Function` node (declarations, expressions, methods).
    pub(super) fn compile_fn_value(
        &mut self,
        function: &Function,
        options: FnOptions,
    ) -> CompileResult {
        if function.is_generator {
            return unsupported("generator", function.span);
        }
        let params: Vec<&Pat> = function.params.iter().map(|p| &p.pat).collect();
        let body = match &function.body {
            Some(body) => FnBody::Block(&body.stmts),
            None => FnBody::Block(&[]),
        };
        // a function value exists from here (declarations are hoisted: they set no anchor)
        if !self.hoisting_fn {
            self.next_anchor = Some(function.span.lo);
        }
        let options = FnOptions {
            is_async: function.is_async,
            ..options
        };
        self.compile_closure(&params, &body, options)
    }

    pub(super) fn compile_arrow(
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
        self.next_anchor = Some(arrow.span.lo);
        let options = FnOptions {
            name,
            internal,
            is_async: arrow.is_async,
            ..FnOptions::new(FnKind::Arrow)
        };
        self.compile_closure(&params, &body, options)
    }

    /// Push a function only the VM calls (class field initialisers, static blocks): no
    /// parameters, no name, no host wrapper.
    pub(super) fn compile_hidden_method(&mut self, body: &FnBody) -> CompileResult {
        let options = FnOptions {
            internal: true,
            ..FnOptions::new(FnKind::Method)
        };
        self.compile_closure(&[], body, options)
    }

    /// Compile a function body in its own compiler and push the closure.
    pub(super) fn compile_closure(
        &mut self,
        params: &[&Pat],
        body: &FnBody,
        options: FnOptions,
    ) -> CompileResult {
        let created_at = self
            .next_anchor
            .take()
            .or(self.class_anchor)
            .unwrap_or(BytePos(0));
        let mut c = FnCompiler::new_enclosing(self);
        c.is_arrow = options.kind == FnKind::Arrow;
        c.is_async = options.is_async;
        c.created_at = created_at;
        let body_stmts: Vec<&Stmt> = match body {
            FnBody::Block(stmts) => stmts.iter().collect(),
            _ => Vec::new(),
        };
        c.captured_names = captured_names(params, body, &body_stmts);
        // a base class constructor first sets up the instance's private methods and fields
        // (before its parameters); a derived one does it when `super(...)` returns
        if options.kind == FnKind::BaseClass {
            c.emit_op(Operation::InitInstance);
            c.emit_op(Operation::Pop);
        }
        c.compile_params(params)?;
        if let Some(name) = options.self_name {
            c.bind_self_name(name)?;
        }
        c.compile_fn_body(body, &body_stmts)?;
        c.emit_op(Operation::Return);
        c.patch_tdz(0);

        // `length` counts the parameters before the first default or rest; the VM fills a
        // slot from the arguments for each parameter before the rest
        let is_rest = |p: &&Pat| matches!(p, Pat::Rest(_));
        let length = params
            .iter()
            .position(|p| matches!(p, Pat::Assign(_) | Pat::Rest(_)))
            .unwrap_or(params.len());
        let param_slots = params.iter().take_while(|p| !is_rest(p)).count();
        // an internal function's name and length can't be observed, so leave the name out
        let name = match options.name {
            Some(name) if !options.internal => name.to_string(),
            _ => String::new(),
        };
        let captures = c.upvalues.into_inner();
        self.emit_op(Operation::Function(FnInfo {
            length: length as u8,
            params: param_slots as u8,
            kind: options.kind,
            is_async: options.is_async,
            internal: options.internal,
            name,
            captures,
            code: c.bytecode,
        }));
        Ok(())
    }

    /// Parameters are the first slots; the VM fills them from the call's arguments. A
    /// pattern parameter gets a hidden slot that is destructured once all names exist.
    fn compile_params(&mut self, params: &[&Pat]) -> CompileResult {
        let mut inits = Vec::new();
        let mut rest = None;
        for (i, pat) in params.iter().enumerate() {
            let (target, default) = match pat {
                Pat::Assign(assign) => (&*assign.left, Some(&*assign.right)),
                Pat::Rest(rest_pat) => {
                    rest = Some((&*rest_pat.arg, i as u8));
                    break; // the parser only allows it last
                }
                pat => (*pat, None),
            };
            let slot = self.alloc_slot()?;
            match target {
                Pat::Ident(ident) => {
                    if self.add_local(ident.id.sym.to_string(), slot) {
                        self.emit_op(Operation::BoxLocal(slot));
                    }
                    if let Some(default) = default {
                        let name = &*ident.id.sym;
                        inits.push(ParamInit::Default { slot, default, name });
                    }
                }
                pat => inits.push(ParamInit::Pattern { slot, default, pat }),
            }
        }

        // names bound by patterns (and the rest parameter)
        let pattern_params = inits.iter().filter_map(|init| match init {
            ParamInit::Pattern { pat, .. } => Some(*pat),
            ParamInit::Default { .. } => None,
        });
        let bound: Vec<&Pat> = pattern_params.chain(rest.map(|(pat, _)| pat)).collect();
        for pat in bound {
            for ident in pat_idents(pat) {
                self.declare_variable(ident.sym.to_string())?;
            }
        }

        // defaults and patterns run in parameter order; a default is used when its
        // argument is missing or `undefined`
        for init in inits {
            match init {
                ParamInit::Default {
                    slot,
                    default,
                    name,
                } => {
                    self.emit_load_slot(slot);
                    self.emit_default(default, Some(name))?;
                    self.emit_store_slot(slot);
                    self.emit_op(Operation::Pop);
                }
                ParamInit::Pattern { slot, default, pat } => {
                    self.emit_op(Operation::GetLocal(slot));
                    if let Some(default) = default {
                        self.emit_default(default, None)?;
                    }
                    self.compile_pattern(pat, Bind::Init)?;
                }
            }
        }
        if let Some((pat, start)) = rest {
            self.emit_op(Operation::RestArgs(start));
            self.compile_pattern(pat, Bind::Init)?;
        }
        Ok(())
    }

    /// A named function expression's own name: the running function, read-only. A
    /// parameter with the same name wins.
    fn bind_self_name(&mut self, name: &Ident) -> CompileResult {
        if self.locals.iter().any(|l| l.name == *name.sym) {
            return Ok(());
        }
        let slot = self.declare_variable(name.sym.to_string())?;
        self.last_local().read_only = true;
        self.emit_op(Operation::Callee);
        self.emit_store_slot(slot);
        self.emit_op(Operation::Pop);
        Ok(())
    }

    /// The body, ending with the return value on the stack.
    fn compile_fn_body(&mut self, body: &FnBody, body_stmts: &[&Stmt]) -> CompileResult {
        match body {
            FnBody::Block(_) => {
                self.compile_body(body_stmts, BodyKind::Function)?;
                // implicit `return undefined`
                self.emit_op(Operation::Undefined);
            }
            FnBody::Expr(expr) => self.compile_expr(expr)?,
            FnBody::Field(expr, Some(name)) => self.compile_expr_named(expr, name)?,
            FnBody::Field(expr, None) => self.compile_expr(expr)?,
            FnBody::DefaultDerived => {
                self.emit_op(Operation::RestArgs(0));
                self.emit_op(Operation::SpreadArgs);
                self.emit_op(Operation::SuperCall);
                self.emit_op(Operation::SuperCallFinish);
                self.emit_op(Operation::Pop);
                self.emit_op(Operation::Undefined);
            }
            FnBody::InstanceFields(fields) => {
                for (i, field) in fields.iter().enumerate() {
                    match (field.value, &field.name) {
                        (Some(value), Some(name)) => self.compile_expr_named(value, name)?,
                        (Some(value), None) => self.compile_expr(value)?,
                        (None, _) => self.emit_op(Operation::Undefined),
                    }
                    let name_it = field.name.is_none() && field.value.is_some_and(is_anonymous_fn);
                    self.emit_op(Operation::InitField(i as u32, name_it));
                }
                // the instance is what `super(...)` / the constructor's set-up gives back
                self.emit_op(Operation::This);
            }
        }
        Ok(())
    }
}

/// A parameter that needs code at the start of the function.
enum ParamInit<'p> {
    /// `x = default`
    Default {
        slot: u8,
        default: &'p Expr,
        name: &'p str,
    },
    /// `{ a, b }` / `[a, b]`, with an optional default
    Pattern {
        slot: u8,
        default: Option<&'p Expr>,
        pat: &'p Pat,
    },
}
