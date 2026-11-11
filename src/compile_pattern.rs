//! Compiling destructuring patterns.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // destructuring
    // ---------------------------------------------------------------------------------

    /// Push a property key.
    pub(crate) fn compile_prop_name(&mut self, key: &PropName) -> CompileResult {
        match key {
            PropName::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            PropName::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            // the VM converts the number to a key the same way JS does
            PropName::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            PropName::Computed(computed) => self.compile_expr(&computed.expr)?,
            key => return unsupported("object key", key.span()),
        }
        Ok(())
    }

    /// Replace the value on top of the stack with `default` if it is `undefined`. `name`
    /// names an anonymous function default after its target.
    pub(crate) fn emit_default(&mut self, default: &Expr, name: Option<&str>) -> CompileResult {
        self.emit_op(Operation::Dup);
        self.emit_op(Operation::Undefined);
        self.emit_op(Operation::EqEqEq);
        let skip = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::Pop);
        match name {
            Some(name) => self.compile_expr_named(default, name)?,
            None => self.compile_expr(default)?,
        }
        let end = self.emit_jump(Operation::Jump(0));
        self.patch_jump(skip);
        self.emit_op(Operation::Pop);
        self.patch_jump(end);
        Ok(())
    }

    /// Store the value on top of the stack into `pat`, consuming it. `init` initialises
    /// declared names (so `const` is allowed); otherwise this is an assignment.
    pub(crate) fn compile_pattern(&mut self, pat: &Pat, init: bool) -> CompileResult {
        match pat {
            Pat::Ident(ident) => {
                if init {
                    self.emit_init_var(&ident.id)?;
                } else {
                    self.emit_set_var(&ident.id)?;
                }
                self.emit_op(Operation::Pop);
            }
            Pat::Assign(assign) => {
                let name = match &*assign.left {
                    Pat::Ident(ident) => Some(&*ident.id.sym),
                    _ => None,
                };
                self.emit_default(&assign.right, name)?;
                self.compile_pattern(&assign.left, init)?;
            }
            Pat::Array(array_pat) => self.compile_array_pattern(array_pat, init)?,
            Pat::Object(object_pat) => self.compile_object_pattern(object_pat, init)?,
            // assignment targets: `[o.x, (y)] = ...`
            Pat::Expr(expr) => {
                let mut expr = &**expr;
                while let Expr::Paren(paren) = expr {
                    expr = &paren.expr;
                }
                match expr {
                    Expr::Ident(ident) => {
                        self.emit_set_var(ident)?;
                        self.emit_op(Operation::Pop);
                    }
                    Expr::Member(member_expr) => {
                        let value = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(value));
                        self.emit_op(Operation::Pop);
                        self.compile_expr(&member_expr.obj)?;
                        let access = self.compile_member_prop(&member_expr.prop)?;
                        self.emit_op(Operation::GetLocal(value));
                        self.emit_member_set(access);
                        self.emit_op(Operation::Pop);
                    }
                    Expr::SuperProp(super_prop) => {
                        let value = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(value));
                        self.emit_op(Operation::Pop);
                        self.compile_super_prop(&super_prop.prop)?;
                        self.emit_op(Operation::GetLocal(value));
                        self.emit_op(Operation::SuperSet);
                        self.emit_op(Operation::Pop);
                    }
                    expr => return unsupported("assignment target", expr.span()),
                }
            }
            pat => return unsupported("pattern", pat.span()),
        }
        Ok(())
    }

    /// `[a, , b = 1, ...rest]`: the value is iterated (only as far as needed) into an array
    /// held in a hidden slot, then each element is stored.
    pub(crate) fn compile_array_pattern(
        &mut self,
        array_pat: &ArrayPat,
        init: bool,
    ) -> CompileResult {
        let has_rest = matches!(array_pat.elems.last(), Some(Some(Pat::Rest(_))));
        let count = if has_rest {
            ITER_TAKE_ALL
        } else {
            array_pat.elems.len() as u32
        };
        let items = self.alloc_slot()?;
        self.emit_op(Operation::IterTake(count));
        self.emit_op(Operation::SetLocal(items));
        self.emit_op(Operation::Pop);
        for (i, elem) in array_pat.elems.iter().enumerate() {
            let Some(pat) = elem else { continue };
            if let Pat::Rest(rest) = pat {
                self.emit_op(Operation::GetLocal(items));
                self.emit_op(Operation::ArraySliceFrom(i as u32));
                self.compile_pattern(&rest.arg, init)?;
            } else {
                self.emit_op(Operation::GetSlotIndex(items, i as u32));
                self.compile_pattern(pat, init)?;
            }
        }
        Ok(())
    }

    /// `{ a, b: c, [k]: d = 1, ...rest }`: the value is held in a hidden slot while each
    /// property is read and stored. With a rest element, the keys used are collected too.
    pub(crate) fn compile_object_pattern(
        &mut self,
        object_pat: &ObjectPat,
        init: bool,
    ) -> CompileResult {
        let source = self.alloc_slot()?;
        self.emit_op(Operation::RequireObjectCoercible);
        self.emit_op(Operation::SetLocal(source));
        self.emit_op(Operation::Pop);

        let has_rest = object_pat
            .props
            .iter()
            .any(|p| matches!(p, ObjectPatProp::Rest(_)));
        let used_keys = if has_rest {
            let slot = self.alloc_slot()?;
            self.emit_op(Operation::Array(0));
            self.emit_op(Operation::SetLocal(slot));
            self.emit_op(Operation::Pop);
            Some(slot)
        } else {
            None
        };

        for prop in &object_pat.props {
            match prop {
                ObjectPatProp::KeyValue(key_value) if used_keys.is_none() => {
                    // `{ a: x }`: one read
                    match &key_value.key {
                        PropName::Ident(ident) => self.emit_op(Operation::GetSlotProp(
                            source,
                            ident.sym.as_bytes().to_vec(),
                        )),
                        PropName::Str(s) => self
                            .emit_op(Operation::GetSlotProp(source, s.value.as_bytes().to_vec())),
                        key => {
                            self.emit_op(Operation::GetLocal(source));
                            self.compile_prop_name(key)?;
                            self.emit_op(Operation::GetProperty);
                        }
                    }
                    self.compile_pattern(&key_value.value, init)?;
                }
                ObjectPatProp::KeyValue(key_value) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.compile_prop_name(&key_value.key)?;
                    if let Some(used) = used_keys {
                        // [source, key] -> record the key, keep it for the read
                        self.emit_op(Operation::Dup);
                        let key = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(key));
                        self.emit_op(Operation::Pop);
                        self.emit_op(Operation::GetLocal(used));
                        self.emit_op(Operation::GetLocal(key));
                        self.emit_op(Operation::ArrayAppend);
                        self.emit_op(Operation::Pop);
                    }
                    self.emit_op(Operation::GetProperty);
                    self.compile_pattern(&key_value.value, init)?;
                }
                ObjectPatProp::Assign(assign) => {
                    // shorthand `{ a }` / `{ a = 1 }`
                    let name = &assign.key.id;
                    if let Some(used) = used_keys {
                        self.emit_op(Operation::GetLocal(used));
                        self.emit_op(load_str(&name.sym));
                        self.emit_op(Operation::ArrayAppend);
                        self.emit_op(Operation::Pop);
                    }
                    self.emit_op(Operation::GetSlotProp(source, name.sym.as_bytes().to_vec()));
                    if let Some(default) = &assign.value {
                        self.emit_default(default, Some(&name.sym))?;
                    }
                    if init {
                        self.emit_init_var(name)?;
                    } else {
                        self.emit_set_var(name)?;
                    }
                    self.emit_op(Operation::Pop);
                }
                ObjectPatProp::Rest(rest) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.emit_op(Operation::GetLocal(used_keys.expect("rest has a key list")));
                    self.emit_op(Operation::ObjectRest);
                    self.compile_pattern(&rest.arg, init)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn compile_bin_expr(&mut self, expr: &BinExpr) -> CompileResult {
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

    pub(crate) fn compile_unary_expr(&mut self, expr: &UnaryExpr) -> CompileResult {
        if expr.op == UnaryOp::Delete {
            // `delete obj.key` needs the object and key, not the property value
            // `delete a?.b`: true without deleting anything when `a` is null / undefined
            if let Expr::OptChain(chain) = &*expr.arg {
                if let OptChainBase::Member(member) = &*chain.base {
                    let mut nullish = Vec::new();
                    self.compile_chain_part(&member.obj, &mut nullish)?;
                    if chain.optional {
                        nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                    }
                    self.compile_member_prop(&member.prop)?;
                    self.emit_op(Operation::Delete);
                    self.end_chain(nullish, Operation::Bool(true));
                    return Ok(());
                }
            }
            let Expr::Member(member_expr) = &*expr.arg else {
                // `delete 1`, `delete f()`: evaluated for side effects, always true
                self.compile_expr(&expr.arg)?;
                self.emit_op(Operation::Pop);
                self.emit_op(Operation::Bool(true));
                return Ok(());
            };
            if let MemberProp::PrivateName(name) = &member_expr.prop {
                return unsupported("delete of a private name", name.span);
            }
            self.compile_expr(&member_expr.obj)?;
            self.compile_member_prop(&member_expr.prop)?;
            self.emit_op(Operation::Delete);
            return Ok(());
        }

        // `typeof window` must not throw when the host has no `window`
        if expr.op == UnaryOp::TypeOf {
            if let Expr::Ident(ident) = &*expr.arg {
                if let (None, Some(host)) = (
                    self.resolve_local(&ident.sym),
                    HostGlobal::from_name(&ident.sym),
                ) {
                    self.emit_op(Operation::HostGlobal(host as u8 | HOST_GLOBAL_TYPEOF));
                    self.emit_op(Operation::TypeOf);
                    return Ok(());
                }
            }
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

    pub(crate) fn compile_lit(&mut self, lit: &Lit) -> CompileResult {
        match lit {
            Lit::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            Lit::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            Lit::Bool(b) => self.emit_op(Operation::Bool(b.value)),
            Lit::Null(_) => self.emit_op(Operation::Null),
            Lit::Regex(r) => {
                self.emit_op(Operation::Regex(r.exp.to_string(), r.flags.to_string()));
            }
            Lit::BigInt(b) => self.emit_op(Operation::LoadBigInt(b.value.to_string())),
            Lit::JSXText(j) => return unsupported("JSX", j.span),
        }
        Ok(())
    }
}
