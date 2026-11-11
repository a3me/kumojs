//! Compiling destructuring patterns.

use super::prelude::*;

impl FnCompiler<'_> {
    /// Replace the value on top of the stack with `default` if it is `undefined`. `name`
    /// names an anonymous function default after its target.
    pub(super) fn emit_default(&mut self, default: &Expr, name: Option<&str>) -> CompileResult {
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

    /// Store the value on top of the stack into `pat`, consuming it.
    pub(super) fn compile_pattern(&mut self, pat: &Pat, bind: Bind) -> CompileResult {
        match pat {
            Pat::Ident(ident) => {
                self.emit_bind_var(&ident.id, bind)?;
                self.emit_op(Operation::Pop);
            }
            Pat::Assign(assign) => {
                let name = match &*assign.left {
                    Pat::Ident(ident) => Some(&*ident.id.sym),
                    _ => None,
                };
                self.emit_default(&assign.right, name)?;
                self.compile_pattern(&assign.left, bind)?;
            }
            Pat::Array(array_pat) => self.compile_array_pattern(array_pat, bind)?,
            Pat::Object(object_pat) => self.compile_object_pattern(object_pat, bind)?,
            // assignment targets: `[o.x, (y)] = ...`
            Pat::Expr(expr) => {
                match expr.unwrap_parens() {
                    Expr::Ident(ident) => {
                        self.emit_set_var(ident)?;
                        self.emit_op(Operation::Pop);
                    }
                    Expr::Member(member_expr) => {
                        let value = self.alloc_slot()?;
                        self.emit_set_local_pop(value);
                        self.compile_expr(&member_expr.obj)?;
                        let access = self.compile_member_prop(&member_expr.prop)?;
                        self.emit_op(Operation::GetLocal(value));
                        self.emit_member_set(access);
                        self.emit_op(Operation::Pop);
                    }
                    Expr::SuperProp(super_prop) => {
                        let value = self.alloc_slot()?;
                        self.emit_set_local_pop(value);
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
    pub(super) fn compile_array_pattern(
        &mut self,
        array_pat: &ArrayPat,
        bind: Bind,
    ) -> CompileResult {
        let has_rest = matches!(array_pat.elems.last(), Some(Some(Pat::Rest(_))));
        let count = if has_rest {
            ITER_TAKE_ALL
        } else {
            array_pat.elems.len() as u32
        };
        let items = self.alloc_slot()?;
        self.emit_op(Operation::IterTake(count));
        self.emit_set_local_pop(items);
        for (i, elem) in array_pat.elems.iter().enumerate() {
            let Some(pat) = elem else { continue };
            if let Pat::Rest(rest) = pat {
                self.emit_op(Operation::GetLocal(items));
                self.emit_op(Operation::ArraySliceFrom(i as u32));
                self.compile_pattern(&rest.arg, bind)?;
            } else {
                self.emit_op(Operation::GetSlotIndex(items, i as u32));
                self.compile_pattern(pat, bind)?;
            }
        }
        Ok(())
    }

    /// `{ a, b: c, [k]: d = 1, ...rest }`: the value is held in a hidden slot while each
    /// property is read and stored. With a rest element, the keys used are collected too.
    pub(super) fn compile_object_pattern(
        &mut self,
        object_pat: &ObjectPat,
        bind: Bind,
    ) -> CompileResult {
        let source = self.alloc_slot()?;
        self.emit_op(Operation::RequireObjectCoercible);
        self.emit_set_local_pop(source);

        let has_rest = object_pat
            .props
            .iter()
            .any(|p| matches!(p, ObjectPatProp::Rest(_)));
        let used_keys = if has_rest {
            let slot = self.alloc_slot()?;
            self.emit_op(Operation::Array(0));
            self.emit_set_local_pop(slot);
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
                    self.compile_pattern(&key_value.value, bind)?;
                }
                ObjectPatProp::KeyValue(key_value) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.compile_prop_name(&key_value.key)?;
                    if let Some(used) = used_keys {
                        // [source, key] -> record the key, keep it for the read
                        self.emit_op(Operation::Dup);
                        let key = self.alloc_slot()?;
                        self.emit_set_local_pop(key);
                        self.emit_op(Operation::GetLocal(used));
                        self.emit_op(Operation::GetLocal(key));
                        self.emit_op(Operation::ArrayAppend);
                        self.emit_op(Operation::Pop);
                    }
                    self.emit_op(Operation::GetProperty);
                    self.compile_pattern(&key_value.value, bind)?;
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
                    self.emit_bind_var(name, bind)?;
                    self.emit_op(Operation::Pop);
                }
                ObjectPatProp::Rest(rest) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.emit_op(Operation::GetLocal(used_keys.expect("rest has a key list")));
                    self.emit_op(Operation::ObjectRest);
                    self.compile_pattern(&rest.arg, bind)?;
                }
            }
        }
        Ok(())
    }
}
