//! Compiling calls, `new`, arguments, optional chains and `await`.

use super::prelude::*;

impl FnCompiler<'_> {
    pub(super) fn compile_call(&mut self, expr: &CallExpr) -> CompileResult {
        let callee = match &expr.callee {
            Callee::Expr(callee) => callee,
            Callee::Super(_) => {
                self.compile_args(&expr.args)?;
                self.emit_op(Operation::SuperCall);
                self.emit_op(Operation::SuperCallFinish);
                return Ok(());
            }
            Callee::Import(import) => return unsupported("dynamic import", import.span),
        };

        // `obj.f()` (and `(obj.f)()`) call f with obj as `this`
        let is_method = match callee.unwrap_parens() {
            Expr::Fn(fn_expr) => {
                // IIFE `(function () {...})()`: internal unless its own name escapes in its body
                let internal = match &fn_expr.ident {
                    Some(ident) => !fn_escapes_in_function(&ident.sym, &fn_expr.function),
                    None => true,
                };
                self.compile_fn_expr(fn_expr, internal)?;
                false
            }
            Expr::Arrow(arrow) => {
                // `(() => {...})()` can't refer to itself, so it never escapes
                self.compile_arrow(arrow, None, true)?;
                false
            }
            Expr::SuperProp(super_prop) => {
                self.compile_super_method_callee(&super_prop.prop)?;
                true
            }
            Expr::OptChain(chain) if matches!(&*chain.base, OptChainBase::Member(_)) => {
                // `(a?.b)()` keeps `this`: [obj, f], or [undefined, undefined] if short-circuited
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, true)?;
                self.end_chain(nullish, [Operation::Undefined, Operation::Undefined]);
                true
            }
            Expr::Member(member_expr) => {
                self.compile_method_callee(member_expr)?;
                true
            }
            _ => {
                self.compile_expr(callee)?;
                false
            }
        };

        self.compile_args(&expr.args)?;
        self.emit_call(is_method);
        Ok(())
    }

    /// [] -> [obj, f]: the callee of `obj.f(...)`.
    pub(super) fn compile_method_callee(&mut self, member_expr: &MemberExpr) -> CompileResult {
        self.compile_expr(&member_expr.obj)?;
        self.emit_op(Operation::Dup);
        let access = self.compile_member_prop(&member_expr.prop)?;
        self.emit_member_get(access);
        Ok(())
    }

    /// [] -> [this, f]: the callee of `super.f(...)` (the parent's f, with this `this`).
    fn compile_super_method_callee(&mut self, prop: &SuperProp) -> CompileResult {
        self.emit_op(Operation::This);
        self.compile_super_prop(prop)?;
        self.emit_op(Operation::SuperGet);
        Ok(())
    }

    pub(super) fn compile_new(&mut self, expr: &NewExpr) -> CompileResult {
        self.compile_expr(&expr.callee)?;
        self.compile_args(expr.args.as_deref().unwrap_or_default())?;
        self.emit_op(Operation::New);
        Ok(())
    }

    /// Push each argument, then the argument count.
    fn compile_args(&mut self, args: &[ExprOrSpread]) -> CompileResult {
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
    pub(super) fn compile_spread_array(&mut self, elems: &[Option<ExprOrSpread>]) -> CompileResult {
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

    /// [value] -> [result]: suspend until the awaited value settles. A rejection resumes
    /// at the `Throw` right after, a fulfilment just after it.
    pub(super) fn emit_await(&mut self) {
        self.emit_op(Operation::Await);
        self.emit_op(Operation::Throw);
    }

    // ---- optional chaining

    /// Compile one link of an optional chain (`a?.b.c()`). A `?.` that finds null /
    /// undefined jumps (collected in `nullish`) to the end of the whole chain, with one
    /// value on the stack. With `method`, a member link pushes [obj, value] for a call.
    pub(super) fn compile_chain(
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
                assert!(!method, "a call is never used as a method receiver link");
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
                self.emit_call(is_method);
            }
        }
        Ok(())
    }

    /// An expression inside a chain: further links stay part of the same chain.
    pub(super) fn compile_chain_part(
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
    fn compile_chain_callee(&mut self, callee: &Expr, nullish: &mut Vec<usize>) -> CompileResult<bool> {
        match callee {
            Expr::OptChain(chain) if matches!(&*chain.base, OptChainBase::Member(_)) => {
                self.compile_chain(chain, nullish, true)?;
                Ok(true)
            }
            Expr::Member(member_expr) => {
                self.compile_method_callee(member_expr)?;
                Ok(true)
            }
            Expr::SuperProp(super_prop) => {
                self.compile_super_method_callee(&super_prop.prop)?;
                Ok(true)
            }
            _ => {
                self.compile_chain_part(callee, nullish)?;
                Ok(false)
            }
        }
    }

    /// Land a chain's short-circuit jumps: their value is replaced by `results`.
    pub(super) fn end_chain(
        &mut self,
        nullish: Vec<usize>,
        results: impl IntoIterator<Item = Operation>,
    ) {
        if nullish.is_empty() {
            return;
        }
        let end = self.emit_jump(Operation::Jump(0));
        for jump in nullish {
            self.patch_jump(jump);
        }
        self.emit_op(Operation::Pop);
        for result in results {
            self.emit_op(result);
        }
        self.patch_jump(end);
    }
}
