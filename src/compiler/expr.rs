//! Compiling expressions: dispatch, member access, operators and assignment.

use super::prelude::*;

impl FnCompiler<'_> {
    pub(super) fn compile_expr(&mut self, expr: &Expr) -> CompileResult {
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
            Expr::Fn(fn_expr) => self.compile_fn_expr(fn_expr, false)?,
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
            Expr::Await(await_expr) => {
                if !self.is_async {
                    return unsupported("top-level await", await_expr.span);
                }
                self.compile_expr(&await_expr.arg)?;
                self.emit_await();
            }
            Expr::OptChain(chain) => {
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, false)?;
                self.end_chain(nullish, [Operation::Undefined]);
            }
            _ => return unsupported(expr_kind(expr), expr.span()),
        }
        Ok(())
    }

    /// Compile an expression whose value is unused, then drop it. `x++` becomes `++x`
    /// (no old value to keep), and each part of `a, b` is dropped as it goes.
    pub(super) fn compile_discarded(&mut self, expr: &Expr) -> CompileResult {
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

    /// Compile `expr`, naming it `name` if it's an anonymous function (`let f = () => {}`).
    pub(super) fn compile_expr_named(&mut self, expr: &Expr, name: &str) -> CompileResult {
        match expr.unwrap_parens() {
            Expr::Fn(fn_expr) if fn_expr.ident.is_none() => {
                let options = FnOptions {
                    name: Some(name),
                    ..FnOptions::new(FnKind::Normal)
                };
                self.compile_fn_value(&fn_expr.function, options)
            }
            Expr::Arrow(arrow) => self.compile_arrow(arrow, Some(name), false),
            Expr::Class(class_expr) if class_expr.ident.is_none() => {
                self.compile_class(None, &class_expr.class, Some(name))
            }
            _ => self.compile_expr(expr),
        }
    }

    // ---- member access

    pub(super) fn compile_member_expr(&mut self, member_expr: &MemberExpr) -> CompileResult {
        self.compile_expr(&member_expr.obj)?;
        let access = self.compile_member_prop(&member_expr.prop)?;
        self.emit_member_get(access);
        Ok(())
    }

    /// Push the property key of a member expression (or its private name), and return
    /// how to read / write it.
    pub(super) fn compile_member_prop(&mut self, prop: &MemberProp) -> CompileResult<Access> {
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
    fn emit_key_once(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            if !matches!(&*computed.expr, Expr::Lit(_)) {
                self.emit_op(Operation::ToPropertyKey);
            }
        }
    }

    /// [obj, key] -> [value]
    pub(super) fn emit_member_get(&mut self, access: Access) {
        self.emit_op(match access {
            Access::Property => Operation::GetProperty,
            Access::Private(_) => Operation::PrivateGet,
        });
    }

    /// [obj, key, value] -> [value]
    pub(super) fn emit_member_set(&mut self, access: Access) {
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
    pub(super) fn emit_private_name(&mut self, name: &PrivateName) -> CompileResult<PrivateKind> {
        let key = private_key(name);
        let Some(kind) = self.private_kind(&key) else {
            return unsupported(format!("undeclared private name `{key}`"), name.span);
        };
        let var = self
            .resolve_local(&key)
            .expect("a private name with a kind has a slot");
        self.emit_load_ref(var);
        Ok(kind)
    }

    // ---- operators

    fn compile_bin_expr(&mut self, expr: &BinExpr) -> CompileResult {
        // `&&`, `||` and `??` only evaluate the right side when needed
        let short_circuit = match expr.op {
            BinaryOp::LogicalAnd => Some(Operation::JumpIfFalse(0)),
            BinaryOp::LogicalOr => Some(Operation::JumpIfTrue(0)),
            BinaryOp::NullishCoalescing => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        if let Some(jump_op) = short_circuit {
            self.compile_expr(&expr.left)?;
            let end = self.emit_jump(jump_op);
            self.emit_op(Operation::Pop);
            self.compile_expr(&expr.right)?;
            self.patch_jump(end);
            return Ok(());
        }

        // `#x in obj`: a brand check
        if let (BinaryOp::In, Expr::PrivateName(name)) = (expr.op, &*expr.left) {
            self.emit_private_name(name)?;
            self.compile_expr(&expr.right)?;
            self.emit_op(Operation::PrivateIn);
            return Ok(());
        }

        self.compile_expr(&expr.left)?;
        self.compile_expr(&expr.right)?;
        let op = match expr.op {
            BinaryOp::Add => Operation::Add,
            BinaryOp::Sub => Operation::Sub,
            BinaryOp::Mul => Operation::Mul,
            BinaryOp::Div => Operation::Div,
            BinaryOp::Mod => Operation::Mod,
            BinaryOp::EqEq => Operation::EqEq,
            BinaryOp::NotEq => Operation::NotEq,
            BinaryOp::EqEqEq => Operation::EqEqEq,
            BinaryOp::NotEqEq => Operation::NotEqEq,
            BinaryOp::Lt => Operation::Lt,
            BinaryOp::LtEq => Operation::LtEq,
            BinaryOp::Gt => Operation::Gt,
            BinaryOp::GtEq => Operation::GtEq,
            BinaryOp::LShift => Operation::LShift,
            BinaryOp::RShift => Operation::RShift,
            BinaryOp::ZeroFillRShift => Operation::ZeroFillRShift,
            BinaryOp::BitOr => Operation::BitOr,
            BinaryOp::BitXor => Operation::BitXor,
            BinaryOp::BitAnd => Operation::BitAnd,
            BinaryOp::In => Operation::In,
            BinaryOp::InstanceOf => Operation::InstanceOf,
            BinaryOp::Exp => Operation::Exp,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing => {
                unreachable!("handled above")
            }
        };
        self.emit_op(op);
        Ok(())
    }

    fn compile_unary_expr(&mut self, expr: &UnaryExpr) -> CompileResult {
        match expr.op {
            UnaryOp::Delete => return self.compile_delete(&expr.arg),
            // `typeof window` must not throw when the host has no `window`
            UnaryOp::TypeOf => {
                if let Expr::Ident(ident) = &*expr.arg {
                    if let (None, Some(host)) = (
                        self.resolve_local(&ident.sym),
                        HostGlobal::from_name(&ident.sym),
                    ) {
                        self.emit_op(Operation::HostGlobal(host, true));
                        self.emit_op(Operation::TypeOf);
                        return Ok(());
                    }
                }
            }
            _ => {}
        }

        self.compile_expr(&expr.arg)?;
        let op = match expr.op {
            UnaryOp::Minus => Operation::UnaryMinus,
            UnaryOp::Plus => Operation::UnaryPlus,
            UnaryOp::Bang => Operation::LogicalNot,
            UnaryOp::Tilde => Operation::BitwiseNot,
            UnaryOp::TypeOf => Operation::TypeOf,
            UnaryOp::Void => Operation::Void,
            UnaryOp::Delete => unreachable!("handled above"),
        };
        self.emit_op(op);
        Ok(())
    }

    /// `delete obj.key` needs the object and key, not the property value.
    fn compile_delete(&mut self, arg: &Expr) -> CompileResult {
        // `delete a?.b`: true without deleting anything when `a` is null / undefined
        if let Expr::OptChain(chain) = arg {
            if let OptChainBase::Member(member) = &*chain.base {
                let mut nullish = Vec::new();
                self.compile_chain_part(&member.obj, &mut nullish)?;
                if chain.optional {
                    nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                }
                self.compile_member_prop(&member.prop)?;
                self.emit_op(Operation::Delete);
                self.end_chain(nullish, [Operation::Bool(true)]);
                return Ok(());
            }
        }
        match arg {
            Expr::Member(member_expr) => {
                if let MemberProp::PrivateName(name) = &member_expr.prop {
                    return unsupported("delete of a private name", name.span);
                }
                self.compile_expr(&member_expr.obj)?;
                self.compile_member_prop(&member_expr.prop)?;
                self.emit_op(Operation::Delete);
            }
            // `delete 1`, `delete f()`: evaluated for side effects, always true
            _ => {
                self.compile_expr(arg)?;
                self.emit_op(Operation::Pop);
                self.emit_op(Operation::Bool(true));
            }
        }
        Ok(())
    }

    fn compile_cond_expr(&mut self, expr: &CondExpr) -> CompileResult {
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

    // ---- assignment

    fn compile_assign_expr(&mut self, expr: &AssignExpr) -> CompileResult {
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
                self.compile_array_pattern(array_pat, Bind::Assign)?;
            }
            AssignTarget::Pat(AssignTargetPat::Object(object_pat)) => {
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::Dup);
                self.compile_object_pattern(object_pat, Bind::Assign)?;
            }
            target => return unsupported("assignment target", target.span()),
        }
        Ok(())
    }

    /// `x op= y`. `&&=`, `||=` and `??=` only evaluate and assign the right side when needed.
    fn compile_compound_assign(&mut self, expr: &AssignExpr) -> CompileResult {
        let Some(target) = PlaceExpr::of_assign_target(&expr.left) else {
            return unsupported("compound assignment target", expr.left.span());
        };
        let place = self.compile_place_get(target)?;
        let Some(skip_op) = logical_assign_jump(expr.op) else {
            self.compile_expr(&expr.right)?;
            self.emit_op(arithmetic_assign_op(expr.op));
            return self.emit_place_set(place);
        };

        let skip = self.emit_jump(skip_op);
        self.emit_op(Operation::Pop);
        match place {
            Place::Var(ident) => self.compile_expr_named(&expr.right, &ident.sym)?,
            _ => self.compile_expr(&expr.right)?,
        }
        self.emit_place_set(place)?;
        if place.ref_len() == 0 {
            self.patch_jump(skip);
            return Ok(());
        }
        let end = self.emit_jump(Operation::Jump(0));
        // not assigning: drop the reference from under the value
        self.patch_jump(skip);
        let temp = self.temp()?;
        self.emit_set_local_pop(temp);
        for _ in 0..place.ref_len() {
            self.emit_op(Operation::Pop);
        }
        self.emit_op(Operation::GetLocal(temp));
        self.patch_jump(end);
        Ok(())
    }

    /// `++` / `--` convert to a number (or keep a BigInt) and step by one.
    fn compile_update_expr(&mut self, expr: &UpdateExpr) -> CompileResult {
        let Some(target) = PlaceExpr::of_expr(expr.arg.unwrap_parens()) else {
            return unsupported("update target", expr.arg.span());
        };
        let place = self.compile_place_get(target)?;
        let postfix = !expr.prefix;
        let mut saved = None;
        if postfix {
            // the result is the old value, converted
            self.emit_op(Operation::ToNumeric);
            if place.ref_len() == 0 {
                self.emit_op(Operation::Dup);
            } else {
                let temp = self.temp()?;
                self.emit_op(Operation::SetLocal(temp));
                saved = Some(temp);
            }
        }
        self.emit_op(match expr.op {
            UpdateOp::PlusPlus => Operation::Increment,
            UpdateOp::MinusMinus => Operation::Decrement,
        });
        self.emit_place_set(place)?;
        if postfix {
            self.emit_op(Operation::Pop);
            if let Some(temp) = saved {
                self.emit_op(Operation::GetLocal(temp));
            }
        }
        Ok(())
    }

    /// Push a place's reference (left on the stack for `emit_place_set`) and its value.
    fn compile_place_get<'e>(&mut self, target: PlaceExpr<'e>) -> CompileResult<Place<'e>> {
        Ok(match target {
            PlaceExpr::Var(ident) => {
                self.emit_get_var(ident)?;
                Place::Var(ident)
            }
            PlaceExpr::Member(member_expr) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.emit_key_once(&member_expr.prop);
                self.emit_op(Operation::Dup2);
                self.emit_member_get(access);
                Place::Member(access)
            }
            PlaceExpr::Super(super_prop) => {
                // [key] -> [key, key] -> [key, value]
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::Dup);
                self.emit_op(Operation::SuperGet);
                Place::Super
            }
        })
    }

    /// [reference, value] -> [value]: store into a place read by `compile_place_get`.
    fn emit_place_set(&mut self, place: Place) -> CompileResult {
        match place {
            Place::Var(ident) => self.emit_set_var(ident)?,
            Place::Member(access) => self.emit_member_set(access),
            Place::Super => self.emit_op(Operation::SuperSet),
        }
        Ok(())
    }
}

/// The target of an update or compound assignment, before it's compiled.
#[derive(Clone, Copy)]
enum PlaceExpr<'e> {
    Var(&'e Ident),
    Member(&'e MemberExpr),
    Super(&'e SuperPropExpr),
}

impl<'e> PlaceExpr<'e> {
    fn of_expr(expr: &'e Expr) -> Option<Self> {
        match expr {
            Expr::Ident(ident) => Some(PlaceExpr::Var(ident)),
            Expr::Member(member) => Some(PlaceExpr::Member(member)),
            Expr::SuperProp(super_prop) => Some(PlaceExpr::Super(super_prop)),
            _ => None,
        }
    }

    fn of_assign_target(target: &'e AssignTarget) -> Option<Self> {
        match target {
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => Some(PlaceExpr::Var(&ident.id)),
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => Some(PlaceExpr::Member(member)),
            AssignTarget::Simple(SimpleAssignTarget::SuperProp(super_prop)) => {
                Some(PlaceExpr::Super(super_prop))
            }
            _ => None,
        }
    }
}

/// A place whose reference is on the stack (under its value), ready to be written.
#[derive(Clone, Copy)]
enum Place<'e> {
    /// nothing on the stack: the variable is written by name
    Var(&'e Ident),
    /// [obj, key]
    Member(Access),
    /// [key]
    Super,
}

impl Place<'_> {
    /// How many values the reference takes up on the stack.
    fn ref_len(&self) -> usize {
        match self {
            Place::Var(_) => 0,
            Place::Member(_) => 2,
            Place::Super => 1,
        }
    }
}

/// `&&=` / `||=` / `??=`: the jump that skips the assignment.
fn logical_assign_jump(op: AssignOp) -> Option<Operation> {
    match op {
        AssignOp::AndAssign => Some(Operation::JumpIfFalse(0)),
        AssignOp::OrAssign => Some(Operation::JumpIfTrue(0)),
        AssignOp::NullishAssign => Some(Operation::JumpIfNotNullish(0)),
        _ => None,
    }
}

/// `+=`, `-=`, ...: the operator applied before storing.
fn arithmetic_assign_op(op: AssignOp) -> Operation {
    match op {
        AssignOp::AddAssign => Operation::Add,
        AssignOp::SubAssign => Operation::Sub,
        AssignOp::MulAssign => Operation::Mul,
        AssignOp::DivAssign => Operation::Div,
        AssignOp::ModAssign => Operation::Mod,
        AssignOp::LShiftAssign => Operation::LShift,
        AssignOp::RShiftAssign => Operation::RShift,
        AssignOp::ZeroFillRShiftAssign => Operation::ZeroFillRShift,
        AssignOp::BitOrAssign => Operation::BitOr,
        AssignOp::BitXorAssign => Operation::BitXor,
        AssignOp::BitAndAssign => Operation::BitAnd,
        AssignOp::ExpAssign => Operation::Exp,
        AssignOp::Assign | AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign => {
            unreachable!("not an arithmetic assignment")
        }
    }
}

/// What an unsupported expression is called in the error.
fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Arrow(_) => "arrow function",
        Expr::Class(_) => "class expression",
        Expr::Tpl(_) | Expr::TaggedTpl(_) => "template literal",
        Expr::SuperProp(_) => "super property",
        Expr::MetaProp(_) => "meta property",
        _ => "expression",
    }
}
