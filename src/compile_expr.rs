//! Compiling expressions (incl. generators/async, optional chaining, templates).

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // expressions
    // ---------------------------------------------------------------------------------

    /// Push the property key of a member expression (or its private name), and return
    /// how to read / write it.
    pub(crate) fn compile_member_prop(&mut self, prop: &MemberProp) -> CompileResult<Access> {
        match prop {
            MemberProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            MemberProp::Computed(computed) => self.compile_expr(&computed.expr)?,
            MemberProp::PrivateName(name) => {
                return Ok(Access::Private(self.emit_private_name(name)?))
            }
        }
        Ok(Access::Property)
    }

    /// A computed key that's both read and written (`o[k] += 1`, `o[k]++`) is converted to
    /// a property key once, so `k`'s `toString` runs once.
    pub(crate) fn emit_key_once(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            if !matches!(&*computed.expr, Expr::Lit(_)) {
                self.emit_op(Operation::ToPropertyKey);
            }
        }
    }

    /// [obj, key] -> [value]
    pub(crate) fn emit_member_get(&mut self, access: Access) {
        self.emit_op(match access {
            Access::Property => Operation::GetProperty,
            Access::Private(_) => Operation::PrivateGet,
        });
    }

    /// [obj, key, value] -> [value]
    pub(crate) fn emit_member_set(&mut self, access: Access) {
        match access {
            Access::Property => self.emit_op(Operation::SetProperty),
            Access::Private(kind) => {
                self.emit_op(Operation::PrivateSet);
                if kind == PrivateKind::Accessor {
                    // drop the setter's return value
                    self.emit_op(Operation::Pop);
                }
            }
        }
    }

    /// Push the private name object `#x` refers to (from the enclosing class bodies).
    pub(crate) fn emit_private_name(&mut self, name: &PrivateName) -> CompileResult<PrivateKind> {
        let key = private_key(name);
        let Some(kind) = self.private_kind(&key) else {
            return unsupported(format!("undeclared private name `{key}`"), name.span);
        };
        match self.resolve_local(&key) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::GetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::GetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::GetUpvalue(index)),
            None => unreachable!("a private name with a kind has a slot"),
        }
        Ok(kind)
    }

    /// The kind of the innermost private name called `key` (`#x`) in scope.
    pub(crate) fn private_kind(&self, key: &str) -> Option<PrivateKind> {
        match self.locals.iter().rev().find(|l| l.name == key) {
            Some(local) => local.private,
            None => self.enclosing?.private_kind(key),
        }
    }

    pub(crate) fn compile_member_expr(&mut self, member_expr: &MemberExpr) -> CompileResult {
        self.compile_expr(&member_expr.obj)?;
        let access = self.compile_member_prop(&member_expr.prop)?;
        self.emit_member_get(access);
        Ok(())
    }

    pub(crate) fn compile_expr(&mut self, expr: &Expr) -> CompileResult {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit)?,
            Expr::Call(call_expr) => self.compile_call(call_expr)?,
            Expr::New(new_expr) => self.compile_new(new_expr)?,
            Expr::Member(member_expr) => self.compile_member_expr(member_expr)?,
            Expr::Ident(ident) => self.emit_get_var(ident)?,
            Expr::This(_) => self.emit_op(Operation::This),
            Expr::Bin(bin_expr) => self.compile_bin_expr(bin_expr)?,
            Expr::Unary(unary_expr) => self.compile_unary_expr(unary_expr)?,
            Expr::Update(update_expr) => self.compile_update_expr(update_expr)?,
            Expr::Paren(paren_expr) => self.compile_expr(&paren_expr.expr)?,
            Expr::Assign(assign_expr) => self.compile_assign_expr(assign_expr)?,
            Expr::Array(array_lit) => self.compile_array_lit(array_lit)?,
            Expr::Object(object_lit) => self.compile_object_lit(object_lit)?,
            Expr::Cond(cond_expr) => self.compile_cond_expr(cond_expr)?,
            Expr::Seq(seq_expr) => {
                let last = seq_expr.exprs.len() - 1;
                for (i, expr) in seq_expr.exprs.iter().enumerate() {
                    self.compile_expr(expr)?;
                    if i != last {
                        self.emit_op(Operation::Pop);
                    }
                }
            }
            Expr::Fn(fn_expr) => {
                self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, false)?
            }
            Expr::Arrow(arrow) => self.compile_arrow(arrow, None, false)?,
            Expr::Tpl(tpl) => self.compile_tpl(tpl)?,
            Expr::Class(class_expr) => {
                let name = class_expr.ident.as_ref().map(|i| &*i.sym);
                self.compile_class(class_expr.ident.as_ref(), &class_expr.class, name)?
            }
            Expr::SuperProp(super_prop) => {
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::SuperGet);
            }
            Expr::MetaProp(meta) if meta.kind == MetaPropKind::NewTarget => {
                self.emit_op(Operation::NewTarget)
            }
            Expr::TaggedTpl(tagged) => self.compile_tagged_tpl(tagged)?,
            Expr::Yield(yield_expr) => self.compile_yield(yield_expr)?,
            Expr::Await(await_expr) => {
                if !self.coroutine.is_async() {
                    return unsupported("top-level await", await_expr.span);
                }
                self.compile_expr(&await_expr.arg)?;
                self.emit_await();
            }
            Expr::OptChain(chain) => {
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, false)?;
                self.end_chain(nullish, Operation::Undefined);
            }
            _ => return unsupported(expr_kind(expr), expr.span()),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // generators and async functions
    // ---------------------------------------------------------------------------------

    /// [value] -> [result]: suspend until the awaited value settles. A rejection resumes
    /// at the `Throw` right after, a fulfilment just after it.
    pub(crate) fn emit_await(&mut self) {
        self.emit_op(Operation::Await);
        self.emit_op(Operation::Throw);
    }

    /// Emit `Yield` for the value on top of the stack. `raw` yields it as the iterator
    /// result itself (`yield*` passing on its inner results). Resuming continues after it
    /// with the value sent in by `next(v)`; `throw(e)` throws e here and `return(v)`
    /// returns v from here (running `finally` blocks and closing `for-of` iterators).
    pub(crate) fn emit_yield(&mut self, raw: bool) -> CompileResult {
        let [normal, throw, ret] = self.emit_multi_jump(Operation::Yield(raw as u8));
        self.patch_from_end(throw);
        self.emit_op(Operation::Throw);
        self.patch_from_end(ret);
        self.emit_generator_return()?;
        self.patch_from_end(normal);
        Ok(())
    }

    /// Emit an instruction ending in N u32 forward offsets, each counted from the end of
    /// the instruction. Returns (instruction end, operand position) pairs to patch.
    pub(crate) fn emit_multi_jump<const N: usize>(&mut self, op: Operation) -> [(usize, usize); N] {
        self.emit_op(op);
        let end = self.bytecode.len();
        std::array::from_fn(|i| (end, end - 4 * (N - i)))
    }

    /// Point an offset from `emit_multi_jump` at the current end of the bytecode.
    pub(crate) fn patch_from_end(&mut self, (end, pos): (usize, usize)) {
        let offset = (self.bytecode.len() - end) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// [value] -> return it from the generator (an async generator awaits it first).
    pub(crate) fn emit_generator_return(&mut self) -> CompileResult {
        if self.coroutine == Coroutine::AsyncGenerator {
            self.emit_await();
        }
        self.emit_unwind_tries(0)?;
        self.emit_op(Operation::Return);
        Ok(())
    }

    pub(crate) fn compile_yield(&mut self, expr: &YieldExpr) -> CompileResult {
        if !expr.delegate {
            match &expr.arg {
                Some(arg) => self.compile_expr(arg)?,
                None => self.emit_op(Operation::Undefined),
            }
            // an async generator awaits what it yields
            if self.coroutine == Coroutine::AsyncGenerator {
                self.emit_await();
            }
            return self.emit_yield(false);
        }

        // `yield* inner`: pass each next / throw / return on to inner's iterator, yielding
        // its results, until it's done (its final value is the result)
        let is_async = self.coroutine.is_async();
        let iterator = self.alloc_slot()?;
        let mode = self.alloc_slot()?;
        let arg = expr.arg.as_deref().expect("yield* always has an argument");
        self.compile_expr(arg)?;
        self.emit_op(if is_async {
            Operation::GetAsyncIterator
        } else {
            Operation::GetIterator
        });
        self.emit_op(Operation::SetLocal(iterator));
        self.emit_op(Operation::Pop);
        self.emit_set_mode(mode, 0);
        // the first value passed on is `undefined`
        self.emit_op(Operation::Undefined);

        let start = self.bytecode.len();
        // [received] -> [inner result]
        let no_return = self.emit_jump(Operation::DelegateCall(iterator, mode, 0));
        if is_async {
            self.emit_await();
        }
        // done: [value] -> the result, or the generator's return value (mode 2)
        let [done, ret] = self.emit_multi_jump(Operation::DelegateResult(mode, is_async as u8));
        let [normal, throw, resumed_return] =
            self.emit_multi_jump(Operation::Yield(!is_async as u8));
        for (resume, m) in [(throw, 1), (resumed_return, 2), (normal, 0)] {
            self.patch_from_end(resume);
            self.emit_set_mode(mode, m);
            self.emit_loop(start);
        }

        // inner is done with a `return(v)`, or has no `return`: return v from here
        self.patch_jump(no_return);
        self.patch_from_end(ret);
        self.emit_generator_return()?;
        self.patch_from_end(done);
        Ok(())
    }

    /// Store a `yield*` resume mode (0 next, 1 throw, 2 return) in its slot.
    pub(crate) fn emit_set_mode(&mut self, slot: u8, mode: u8) {
        self.emit_op(Operation::LoadFloat64(mode as f64));
        self.emit_op(Operation::SetLocal(slot));
        self.emit_op(Operation::Pop);
    }

    // ---------------------------------------------------------------------------------
    // optional chaining
    // ---------------------------------------------------------------------------------

    /// Compile one link of an optional chain (`a?.b.c()`). A `?.` that finds null /
    /// undefined jumps (collected in `nullish`) to the end of the whole chain, with one
    /// value on the stack. With `method`, a member link pushes [obj, value] for a call.
    pub(crate) fn compile_chain(
        &mut self,
        chain: &OptChainExpr,
        nullish: &mut Vec<usize>,
        method: bool,
    ) -> CompileResult {
        match &*chain.base {
            OptChainBase::Member(member) => {
                self.compile_chain_part(&member.obj, nullish)?;
                if chain.optional {
                    nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                }
                if method {
                    self.emit_op(Operation::Dup);
                }
                let access = self.compile_member_prop(&member.prop)?;
                self.emit_member_get(access);
            }
            OptChainBase::Call(call) => {
                let is_method = self.compile_chain_callee(&call.callee, nullish)?;
                if chain.optional {
                    if is_method {
                        // [obj, f]: leave one value for the end of the chain
                        let call_it = self.emit_jump(Operation::JumpIfNotNullish(0));
                        self.emit_op(Operation::Pop);
                        nullish.push(self.emit_jump(Operation::Jump(0)));
                        self.patch_jump(call_it);
                    } else {
                        nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                    }
                }
                self.compile_args(&call.args)?;
                self.emit_op(if is_method {
                    Operation::CallMethod
                } else {
                    Operation::Call
                });
                if method {
                    unreachable!("a call is never used as a method receiver link");
                }
            }
        }
        Ok(())
    }

    /// An expression inside a chain: further links stay part of the same chain.
    pub(crate) fn compile_chain_part(
        &mut self,
        expr: &Expr,
        nullish: &mut Vec<usize>,
    ) -> CompileResult {
        match expr {
            Expr::OptChain(chain) => self.compile_chain(chain, nullish, false),
            _ => self.compile_expr(expr),
        }
    }

    /// Push a chain call's callee: [obj, f] for a method call (returns true), else [f].
    pub(crate) fn compile_chain_callee(
        &mut self,
        callee: &Expr,
        nullish: &mut Vec<usize>,
    ) -> CompileResult<bool> {
        match callee {
            Expr::OptChain(chain) if matches!(&*chain.base, OptChainBase::Member(_)) => {
                self.compile_chain(chain, nullish, true)?;
                Ok(true)
            }
            Expr::Member(member) => {
                self.compile_expr(&member.obj)?;
                self.emit_op(Operation::Dup);
                let access = self.compile_member_prop(&member.prop)?;
                self.emit_member_get(access);
                Ok(true)
            }
            Expr::SuperProp(super_prop) => {
                self.emit_op(Operation::This);
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::SuperGet);
                Ok(true)
            }
            _ => {
                self.compile_chain_part(callee, nullish)?;
                Ok(false)
            }
        }
    }

    /// Land a chain's short-circuit jumps: their value is replaced by `result`.
    pub(crate) fn end_chain(&mut self, nullish: Vec<usize>, result: Operation) {
        if nullish.is_empty() {
            return;
        }
        let end = self.emit_jump(Operation::Jump(0));
        for jump in nullish {
            self.patch_jump(jump);
        }
        self.emit_op(Operation::Pop);
        self.emit_op(result);
        self.patch_jump(end);
    }

    /// Compile an expression whose value is unused, then drop it. `x++` becomes `++x`
    /// (no old value to keep), and each part of `a, b` is dropped as it goes.
    pub(crate) fn compile_discarded(&mut self, expr: &Expr) -> CompileResult {
        match expr {
            Expr::Update(update) if !update.prefix => {
                let prefix = UpdateExpr {
                    prefix: true,
                    ..update.clone()
                };
                self.compile_update_expr(&prefix)?;
            }
            Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.compile_discarded(expr)?;
                }
                return Ok(());
            }
            _ => self.compile_expr(expr)?,
        }
        self.emit_op(Operation::Pop);
        Ok(())
    }

    pub(crate) fn compile_cond_expr(&mut self, expr: &CondExpr) -> CompileResult {
        self.compile_expr(&expr.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.cons)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.alt)?;
        self.patch_jump(end_jump);
        Ok(())
    }

    pub(crate) fn compile_update_expr(&mut self, expr: &UpdateExpr) -> CompileResult {
        // `++` / `--` convert to a number (or keep a BigInt) and step by one
        let step = || match expr.op {
            UpdateOp::PlusPlus => Operation::Increment,
            UpdateOp::MinusMinus => Operation::Decrement,
        };
        let mut arg = &*expr.arg;
        while let Expr::Paren(paren) = arg {
            arg = &paren.expr;
        }
        match arg {
            Expr::Ident(ident) => {
                self.emit_get_var(ident)?;
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                if !expr.prefix {
                    // keep the old (numeric) value as the result
                    self.emit_op(Operation::Dup);
                }
                self.emit_op(step());
                self.emit_set_var(ident)?;
                if !expr.prefix {
                    self.emit_op(Operation::Pop);
                }
            }
            Expr::Member(member_expr) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.emit_key_once(&member_expr.prop);
                self.emit_op(Operation::Dup2);
                self.emit_member_get(access);
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                let temp = if expr.prefix {
                    None
                } else {
                    Some(self.temp()?)
                };
                if let Some(temp) = temp {
                    self.emit_op(Operation::SetLocal(temp));
                }
                self.emit_op(step());
                self.emit_member_set(access);
                if let Some(temp) = temp {
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                }
            }
            Expr::SuperProp(super_prop) => {
                // [key] -> [key, key] -> [key, value]
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::Dup);
                self.emit_op(Operation::SuperGet);
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                let temp = if expr.prefix {
                    None
                } else {
                    Some(self.temp()?)
                };
                if let Some(temp) = temp {
                    self.emit_op(Operation::SetLocal(temp));
                }
                self.emit_op(step());
                self.emit_op(Operation::SuperSet);
                if let Some(temp) = temp {
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                }
            }
            _ => return unsupported("update target", expr.arg.span()),
        }
        Ok(())
    }

    pub(crate) fn compile_assign_expr(&mut self, expr: &AssignExpr) -> CompileResult {
        if expr.op != AssignOp::Assign {
            return self.compile_compound_assign(expr);
        }
        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // stack: [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.compile_expr(&expr.right)?;
                self.emit_member_set(access);
            }
            AssignTarget::Simple(SimpleAssignTarget::SuperProp(super_prop)) => {
                self.compile_super_prop(&super_prop.prop)?;
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::SuperSet);
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                self.compile_expr_named(&expr.right, &ident.id.sym)?;
                self.emit_set_var(&ident.id)?;
            }
            // the expression's value is the right-hand side
            AssignTarget::Pat(AssignTargetPat::Array(array_pat)) => {
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::Dup);
                self.compile_array_pattern(array_pat, false)?;
            }
            AssignTarget::Pat(AssignTargetPat::Object(object_pat)) => {
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::Dup);
                self.compile_object_pattern(object_pat, false)?;
            }
            target => return unsupported("assignment target", target.span()),
        }
        Ok(())
    }

    pub(crate) fn compile_compound_assign(&mut self, expr: &AssignExpr) -> CompileResult {
        // `&&=`, `||=` and `??=` only evaluate and assign the RHS when needed
        let short_circuit = match expr.op {
            AssignOp::AndAssign => Some(Operation::JumpIfFalse(0)),
            AssignOp::OrAssign => Some(Operation::JumpIfTrue(0)),
            AssignOp::NullishAssign => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        let arithmetic = match expr.op {
            AssignOp::AddAssign => Some(Operation::Add),
            AssignOp::SubAssign => Some(Operation::Sub),
            AssignOp::MulAssign => Some(Operation::Mul),
            AssignOp::DivAssign => Some(Operation::Div),
            AssignOp::ModAssign => Some(Operation::Mod),
            AssignOp::LShiftAssign => Some(Operation::LShift),
            AssignOp::RShiftAssign => Some(Operation::RShift),
            AssignOp::ZeroFillRShiftAssign => Some(Operation::ZeroFillRShift),
            AssignOp::BitOrAssign => Some(Operation::BitOr),
            AssignOp::BitXorAssign => Some(Operation::BitXor),
            AssignOp::BitAndAssign => Some(Operation::BitAnd),
            AssignOp::ExpAssign => Some(Operation::Exp),
            _ => None,
        };

        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                let ident = &ident.id;
                self.emit_get_var(ident)?;
                if let Some(jump_op) = short_circuit {
                    let end = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr_named(&expr.right, &ident.sym)?;
                    self.emit_set_var(ident)?;
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_set_var(ident)?;
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.emit_key_once(&member_expr.prop);
                self.emit_op(Operation::Dup2);
                self.emit_member_get(access);
                if let Some(jump_op) = short_circuit {
                    let keep = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_member_set(access);
                    let end = self.emit_jump(Operation::Jump(0));
                    // not assigning: drop [obj, key] from under the value
                    self.patch_jump(keep);
                    let temp = self.temp()?;
                    self.emit_op(Operation::SetLocal(temp));
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_member_set(access);
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::SuperProp(super_prop)) => {
                // [key] -> [key, key] -> [key, value]
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::Dup);
                self.emit_op(Operation::SuperGet);
                if let Some(jump_op) = short_circuit {
                    let keep = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_op(Operation::SuperSet);
                    let end = self.emit_jump(Operation::Jump(0));
                    // not assigning: drop the key from under the value
                    self.patch_jump(keep);
                    let temp = self.temp()?;
                    self.emit_op(Operation::SetLocal(temp));
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_op(Operation::SuperSet);
                }
            }
            target => return unsupported("compound assignment target", target.span()),
        }
        Ok(())
    }

    pub(crate) fn compile_array_lit(&mut self, array_lit: &ArrayLit) -> CompileResult {
        if array_lit.elems.iter().flatten().any(|e| e.spread.is_some()) {
            return self.compile_spread_array(&array_lit.elems);
        }
        for elem in &array_lit.elems {
            match elem {
                Some(expr_or_spread) => {
                    if let Some(spread) = expr_or_spread.spread {
                        return unsupported("spread element", spread);
                    }
                    self.compile_expr(&expr_or_spread.expr)?;
                }
                None => self.emit_op(Operation::Hole),
            }
        }
        self.emit_op(Operation::Array(array_lit.elems.len() as u32));
        Ok(())
    }

    pub(crate) fn compile_object_lit(&mut self, object_lit: &ObjectLit) -> CompileResult {
        // leading plain, shorthand and method properties are built in one step; the rest
        // (from the first accessor, spread or `__proto__`) are added one at a time, in order
        let simple = object_lit
            .props
            .iter()
            .take_while(|p| {
                matches!(p, PropOrSpread::Prop(prop)
                    if matches!(&**prop, Prop::Shorthand(_) | Prop::Method(_))
                        || matches!(&**prop, Prop::KeyValue(kv) if !is_proto_key(&kv.key)))
            })
            .count();
        for prop in &object_lit.props[..simple] {
            if let PropOrSpread::Prop(prop) = prop {
                self.compile_prop(prop)?;
            }
        }
        self.emit_op(Operation::Object(simple as u32));
        for prop in &object_lit.props[simple..] {
            let prop = match prop {
                PropOrSpread::Spread(spread) => {
                    self.compile_expr(&spread.expr)?;
                    self.emit_op(Operation::ObjectSpread);
                    continue;
                }
                PropOrSpread::Prop(prop) => &**prop,
            };
            match prop {
                Prop::Getter(getter) => {
                    self.compile_method(&getter.key, &getter.function, 1)?;
                    self.emit_op(Operation::DefineAccessor(0));
                }
                Prop::Setter(setter) => {
                    self.compile_method(&setter.key, &setter.function, 2)?;
                    self.emit_op(Operation::DefineAccessor(1));
                }
                // `__proto__: value` sets the prototype rather than defining a property (and
                // doesn't name an anonymous function value)
                Prop::KeyValue(key_value) if is_proto_key(&key_value.key) => {
                    self.emit_op(Operation::Dup);
                    self.compile_prop_name(&key_value.key)?;
                    self.compile_expr(&key_value.value)?;
                    self.emit_op(Operation::SetProperty);
                    self.emit_op(Operation::Pop);
                }
                _ => {
                    self.compile_prop(prop)?;
                    self.emit_op(Operation::DefineField);
                }
            }
        }
        Ok(())
    }

    /// Push a key-value, shorthand or method property's key and value.
    pub(crate) fn compile_prop(&mut self, prop: &Prop) -> CompileResult {
        match prop {
            Prop::KeyValue(key_value) => {
                self.compile_prop_name(&key_value.key)?;
                match static_key_name(&key_value.key) {
                    Some(name) => self.compile_expr_named(&key_value.value, &name)?,
                    None => {
                        self.compile_expr(&key_value.value)?;
                        if is_anonymous_fn(&key_value.value) {
                            self.emit_op(Operation::SetFunctionName(0));
                        }
                    }
                }
            }
            Prop::Shorthand(ident) => {
                self.emit_op(load_str(&ident.sym));
                self.emit_get_var(ident)?;
            }
            Prop::Method(method) => self.compile_method(&method.key, &method.function, 0)?,
            prop => return unsupported("object property kind", prop.span()),
        }
        Ok(())
    }

    /// Push a method's key and function. `prefix` 1 / 2 names it as a getter / setter.
    pub(crate) fn compile_method(
        &mut self,
        key: &PropName,
        function: &Function,
        prefix: u8,
    ) -> CompileResult {
        self.compile_prop_name(key)?;
        let name = static_key_name(key).map(|name| match prefix {
            1 => format!("get {name}"),
            2 => format!("set {name}"),
            _ => name,
        });
        self.compile_fn_value(name.as_deref(), None, function, false, FnKind::Method)?;
        if name.is_none() {
            self.emit_op(Operation::SetFunctionName(prefix));
        }
        Ok(())
    }

    /// Compile `expr`, naming it `name` if it's an anonymous function (`let f = () => {}`).
    pub(crate) fn compile_expr_named(&mut self, expr: &Expr, name: &str) -> CompileResult {
        let mut unwrapped = expr;
        while let Expr::Paren(paren) = unwrapped {
            unwrapped = &paren.expr;
        }
        match unwrapped {
            Expr::Fn(fn_expr) if fn_expr.ident.is_none() => {
                self.compile_fn_value(Some(name), None, &fn_expr.function, false, FnKind::Normal)
            }
            Expr::Arrow(arrow) => self.compile_arrow(arrow, Some(name), false),
            Expr::Class(class_expr) if class_expr.ident.is_none() => {
                self.compile_class(None, &class_expr.class, Some(name))
            }
            _ => self.compile_expr(expr),
        }
    }

    // ---------------------------------------------------------------------------------
    // templates
    // ---------------------------------------------------------------------------------

    pub(crate) fn compile_tpl(&mut self, tpl: &Tpl) -> CompileResult {
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            let Some(cooked) = &quasi.cooked else {
                return unsupported("invalid escape in template", quasi.span);
            };
            self.emit_op(Operation::LoadString(cooked.as_bytes().to_vec()));
            if let Some(expr) = tpl.exprs.get(i) {
                self.compile_expr(expr)?;
            }
        }
        if !tpl.exprs.is_empty() {
            let parts = tpl.quasis.len() + tpl.exprs.len();
            self.emit_op(Operation::Template(parts as u32));
        }
        Ok(())
    }

    /// ``tag`a${b}c` `` calls `tag(strings, b)`; ``obj.tag`...` `` passes obj as `this`.
    pub(crate) fn compile_tagged_tpl(&mut self, tagged: &TaggedTpl) -> CompileResult {
        let mut tag = &*tagged.tag;
        while let Expr::Paren(paren) = tag {
            tag = &paren.expr;
        }
        let is_method = if let Expr::Member(member_expr) = tag {
            self.compile_expr(&member_expr.obj)?;
            self.emit_op(Operation::Dup);
            let access = self.compile_member_prop(&member_expr.prop)?;
            self.emit_member_get(access);
            true
        } else {
            self.compile_expr(&tagged.tag)?;
            false
        };
        let pieces = tagged
            .tpl
            .quasis
            .iter()
            .map(|q| {
                let cooked = q.cooked.as_ref().map(|c| c.as_bytes().to_vec());
                (cooked, q.raw.as_bytes().to_vec())
            })
            .collect();
        self.emit_op(Operation::TemplateObject(pieces));
        for expr in &tagged.tpl.exprs {
            self.compile_expr(expr)?;
        }
        self.emit_op(Operation::UInt64(tagged.tpl.exprs.len() as u64 + 1));
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
        Ok(())
    }
}
