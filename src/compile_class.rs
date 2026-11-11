//! Compiling classes.

use crate::*;

impl<'a> Compiler<'a> {
    // ---------------------------------------------------------------------------------
    // classes
    // ---------------------------------------------------------------------------------

    /// Push a class. `ident` is the class's own name, bound (read-only) inside its body;
    /// `name` is its `name` property (the own name, or one inferred from where it's stored).
    pub(crate) fn compile_class(
        &mut self,
        ident: Option<&Ident>,
        class: &Class,
        name: Option<&str>,
    ) -> CompileResult {
        let outer = self.class_anchor.replace(class.span.lo);
        let compiled = self.compile_class_body(ident, class, name);
        self.class_anchor = outer;
        compiled
    }

    pub(crate) fn compile_class_body(
        &mut self,
        ident: Option<&Ident>,
        class: &Class,
        name: Option<&str>,
    ) -> CompileResult {
        self.enter_scope();
        let inner = match ident {
            Some(ident) => {
                let slot = self.declare_variable(ident.sym.to_string())?;
                self.locals.last_mut().unwrap().read_only = true;
                // `class A extends A` can't use it; methods (made later) always can
                let body_start = class
                    .super_class
                    .as_ref()
                    .map_or(ident.span.hi, |s| s.span().hi);
                self.mark_tdz(body_start);
                Some(slot)
            }
            None => None,
        };

        // [superclass?, constructor] -> MakeClass -> [class]
        let derived = class.super_class.is_some();
        if let Some(super_class) = &class.super_class {
            self.compile_expr(super_class)?;
        }

        // the body's private names are fresh each time the class is evaluated; the
        // superclass expression above still sees the outer ones
        for (key, kind) in class_private_names(class)? {
            let slot = self.declare_variable(key)?;
            self.locals.last_mut().unwrap().private = Some(kind);
            self.emit_op(Operation::NewPrivateName(kind as u8));
            self.emit_store_slot(slot);
            self.emit_op(Operation::Pop);
        }
        let constructor = class.body.iter().find_map(|m| match m {
            ClassMember::Constructor(c) => Some(c),
            _ => None,
        });
        let kind = if derived {
            FnKind::DerivedClass
        } else {
            FnKind::BaseClass
        };
        match constructor {
            Some(constructor) => {
                let mut params = Vec::new();
                for param in &constructor.params {
                    match param {
                        ParamOrTsParamProp::Param(param) => params.push(&param.pat),
                        ParamOrTsParamProp::TsParamProp(p) => {
                            return unsupported("parameter property", p.span)
                        }
                    }
                }
                let body = match &constructor.body {
                    Some(body) => FnBody::Block(&body.stmts),
                    None => FnBody::Block(&[]),
                };
                self.compile_closure(name, None, &params, &body, false, kind, Coroutine::None)?;
            }
            // `constructor(...args) { super(...args); }` for derived classes
            None if derived => self.compile_closure(
                name,
                None,
                &[],
                &FnBody::DefaultDerived,
                false,
                kind,
                Coroutine::None,
            )?,
            None => self.compile_closure(
                name,
                None,
                &[],
                &FnBody::Block(&[]),
                false,
                kind,
                Coroutine::None,
            )?,
        }
        self.emit_op(Operation::MakeClass(derived as u8));

        // methods and accessors, then instance fields (initialised on construction)
        let mut statics = Vec::new();
        let mut instance_fields = Vec::new();
        for member in &class.body {
            match member {
                ClassMember::Constructor(_) | ClassMember::Empty(_) => {}
                ClassMember::Method(method) => {
                    // [class] -> [class, target]
                    self.emit_op(Operation::Dup);
                    if !method.is_static {
                        self.emit_op(load_str("prototype"));
                        self.emit_op(Operation::GetProperty);
                    }
                    let kind = match method.kind {
                        MethodKind::Method => 0,
                        MethodKind::Getter => 1,
                        MethodKind::Setter => 2,
                    };
                    self.compile_method(&method.key, &method.function, kind)?;
                    self.emit_op(Operation::DefineMethod(kind));
                }
                ClassMember::ClassProp(prop) if prop.is_static => {
                    // a computed key is evaluated (and converted) in source order, its
                    // value later with the other static initialisers
                    let key_slot = match &prop.key {
                        PropName::Computed(_) => {
                            let slot = self.alloc_slot()?;
                            self.compile_prop_name(&prop.key)?;
                            self.emit_op(Operation::ToPropertyKey);
                            self.emit_op(Operation::SetLocal(slot));
                            self.emit_op(Operation::Pop);
                            Some(slot)
                        }
                        _ => None,
                    };
                    statics.push((member, key_slot));
                }
                ClassMember::ClassProp(prop) => {
                    // the key now; the value when an instance is set up
                    self.emit_op(Operation::Dup);
                    self.compile_prop_name(&prop.key)?;
                    self.emit_op(Operation::ClassField);
                    instance_fields.push(FieldInit {
                        value: prop.value.as_deref(),
                        name: static_key_name(&prop.key),
                    });
                }
                ClassMember::StaticBlock(_) => statics.push((member, None)),
                ClassMember::PrivateMethod(method) => {
                    // [class] -> [class, class, name, fn] -> [class]
                    self.emit_op(Operation::Dup);
                    self.emit_private_name(&method.key)?;
                    let key = private_key(&method.key);
                    let (kind, name) = match method.kind {
                        MethodKind::Method => (0, key),
                        MethodKind::Getter => (1, format!("get {key}")),
                        MethodKind::Setter => (2, format!("set {key}")),
                    };
                    // accessors are only ever called by the VM; a method only needs a host
                    // wrapper if it's used as a value (all its uses are in this class body)
                    let internal = kind != 0 || !private_method_escapes(&method.key, class);
                    self.compile_fn_value(
                        Some(&name),
                        None,
                        &method.function,
                        internal,
                        FnKind::Method,
                    )?;
                    let static_flag = if method.is_static { 4 } else { 0 };
                    self.emit_op(Operation::DefinePrivateMethod(kind | static_flag));
                }
                ClassMember::PrivateProp(prop) if prop.is_static => statics.push((member, None)),
                ClassMember::PrivateProp(prop) => {
                    self.emit_op(Operation::Dup);
                    self.emit_private_name(&prop.key)?;
                    self.emit_op(Operation::ClassField);
                    instance_fields.push(FieldInit {
                        value: prop.value.as_deref(),
                        name: Some(private_key(&prop.key)),
                    });
                }
                member => return unsupported("class member", member.span()),
            }
        }

        // all instance field values are set up by one hidden method, run on each new instance
        if !instance_fields.is_empty() {
            self.emit_op(Operation::Dup);
            let body = FnBody::InstanceFields(&instance_fields);
            self.compile_closure(
                None,
                None,
                &[],
                &body,
                true,
                FnKind::Method,
                Coroutine::None,
            )?;
            self.emit_op(Operation::ClassInit);
        }

        // the class's own name is usable from here on (static initialisers can use it)
        if let Some(slot) = inner {
            self.emit_store_slot(slot);
        }

        // static fields and blocks run in order, with `this` as the class
        for (member, key_slot) in statics {
            self.emit_op(Operation::Dup);
            match member {
                ClassMember::ClassProp(prop) => {
                    match key_slot {
                        Some(slot) => self.emit_op(Operation::GetLocal(slot)),
                        None => self.compile_prop_name(&prop.key)?,
                    }
                    self.compile_field_init(static_key_name(&prop.key), prop.value.as_deref())?;
                    self.emit_op(Operation::StaticField);
                }
                ClassMember::PrivateProp(prop) => {
                    self.compile_private_field(prop)?;
                    self.emit_op(Operation::StaticField);
                }
                ClassMember::StaticBlock(block) => {
                    let body = FnBody::Block(&block.body.stmts);
                    self.compile_closure(
                        None,
                        None,
                        &[],
                        &body,
                        true,
                        FnKind::Method,
                        Coroutine::None,
                    )?;
                    self.emit_op(Operation::StaticBlock);
                }
                _ => unreachable!(),
            }
        }
        self.exit_scope();
        Ok(())
    }

    /// Push a private field's name object and initialiser.
    pub(crate) fn compile_private_field(&mut self, prop: &PrivateProp) -> CompileResult {
        self.emit_private_name(&prop.key)?;
        self.compile_field_init(Some(private_key(&prop.key)), prop.value.as_deref())
    }

    /// Push a field's initialiser: a hidden method returning the value (run with `this` as
    /// the instance, or the class for static fields), or `undefined` for `x;`. `name` names
    /// an anonymous function value.
    pub(crate) fn compile_field_init(
        &mut self,
        name: Option<String>,
        value: Option<&Expr>,
    ) -> CompileResult {
        match value {
            Some(value) => {
                let body = FnBody::Field(value, name);
                self.compile_closure(
                    None,
                    None,
                    &[],
                    &body,
                    true,
                    FnKind::Method,
                    Coroutine::None,
                )
            }
            None => {
                self.emit_op(Operation::Undefined);
                Ok(())
            }
        }
    }

    /// Push the key of `super.x` / `super[x]`.
    pub(crate) fn compile_super_prop(&mut self, prop: &SuperProp) -> CompileResult {
        match prop {
            SuperProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            SuperProp::Computed(computed) => self.compile_expr(&computed.expr)?,
        }
        Ok(())
    }
}
