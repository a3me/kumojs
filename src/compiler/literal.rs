//! Compiling literals: primitives, arrays, objects and templates.

use super::prelude::*;

impl FnCompiler<'_> {
    pub(super) fn compile_lit(&mut self, lit: &Lit) -> CompileResult {
        match lit {
            Lit::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            Lit::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            Lit::Bool(b) => self.emit_op(Operation::Bool(b.value)),
            Lit::Null(_) => self.emit_op(Operation::Null),
            Lit::Regex(r) => self.emit_op(Operation::Regex(r.exp.to_string(), r.flags.to_string())),
            Lit::BigInt(b) => self.emit_op(Operation::LoadBigInt(b.value.to_string())),
            Lit::JSXText(j) => return unsupported("JSX", j.span),
        }
        Ok(())
    }

    pub(super) fn compile_array_lit(&mut self, array_lit: &ArrayLit) -> CompileResult {
        if array_lit.elems.iter().flatten().any(|e| e.spread.is_some()) {
            return self.compile_spread_array(&array_lit.elems);
        }
        for elem in &array_lit.elems {
            match elem {
                Some(elem) => self.compile_expr(&elem.expr)?,
                None => self.emit_op(Operation::Hole),
            }
        }
        self.emit_op(Operation::Array(array_lit.elems.len() as u32));
        Ok(())
    }

    pub(super) fn compile_object_lit(&mut self, object_lit: &ObjectLit) -> CompileResult {
        // leading plain, shorthand and method properties are built in one step; the rest
        // (from the first accessor, spread or `__proto__`) are added one at a time, in order
        let simple = object_lit
            .props
            .iter()
            .take_while(|p| match p {
                PropOrSpread::Prop(prop) => match &**prop {
                    Prop::Shorthand(_) | Prop::Method(_) => true,
                    Prop::KeyValue(kv) => !is_proto_key(&kv.key),
                    _ => false,
                },
                PropOrSpread::Spread(_) => false,
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
                    self.compile_method(&getter.key, &getter.function, MethodFlavor::Getter)?;
                    self.emit_op(Operation::DefineAccessor(Accessor::Getter));
                }
                Prop::Setter(setter) => {
                    self.compile_method(&setter.key, &setter.function, MethodFlavor::Setter)?;
                    self.emit_op(Operation::DefineAccessor(Accessor::Setter));
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
    fn compile_prop(&mut self, prop: &Prop) -> CompileResult {
        match prop {
            Prop::KeyValue(key_value) => {
                self.compile_prop_name(&key_value.key)?;
                match static_key_name(&key_value.key) {
                    Some(name) => self.compile_expr_named(&key_value.value, &name)?,
                    None => {
                        self.compile_expr(&key_value.value)?;
                        if is_anonymous_fn(&key_value.value) {
                            self.emit_op(Operation::SetFunctionName(MethodFlavor::Method));
                        }
                    }
                }
            }
            Prop::Shorthand(ident) => {
                self.emit_op(load_str(&ident.sym));
                self.emit_get_var(ident)?;
            }
            Prop::Method(method) => {
                self.compile_method(&method.key, &method.function, MethodFlavor::Method)?
            }
            prop => return unsupported("object property kind", prop.span()),
        }
        Ok(())
    }

    /// Push a method's key and function, named `get x` / `set x` for accessors.
    pub(super) fn compile_method(
        &mut self,
        key: &PropName,
        function: &Function,
        flavor: MethodFlavor,
    ) -> CompileResult {
        self.compile_prop_name(key)?;
        let name = static_key_name(key).map(|name| match flavor {
            MethodFlavor::Method => name,
            MethodFlavor::Getter => format!("get {name}"),
            MethodFlavor::Setter => format!("set {name}"),
        });
        let options = FnOptions {
            name: name.as_deref(),
            ..FnOptions::new(FnKind::Method)
        };
        self.compile_fn_value(function, options)?;
        if name.is_none() {
            // a computed key: named at run time
            self.emit_op(Operation::SetFunctionName(flavor));
        }
        Ok(())
    }

    /// Push a property key.
    pub(super) fn compile_prop_name(&mut self, key: &PropName) -> CompileResult {
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

    // ---- templates

    pub(super) fn compile_tpl(&mut self, tpl: &Tpl) -> CompileResult {
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
    pub(super) fn compile_tagged_tpl(&mut self, tagged: &TaggedTpl) -> CompileResult {
        let is_method = match tagged.tag.unwrap_parens() {
            Expr::Member(member_expr) => {
                self.compile_method_callee(member_expr)?;
                true
            }
            _ => {
                self.compile_expr(&tagged.tag)?;
                false
            }
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
        self.emit_call(is_method);
        Ok(())
    }
}
