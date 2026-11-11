//! Compiling classes.

use super::prelude::*;

impl FnCompiler<'_> {
    /// Push a class. `ident` is the class's own name, bound (read-only) inside its body;
    /// `name` is its `name` property (the own name, or one inferred from where it's stored).
    pub(super) fn compile_class(
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

    fn compile_class_body(
        &mut self,
        ident: Option<&Ident>,
        class: &Class,
        name: Option<&str>,
    ) -> CompileResult {
        self.enter_scope();
        let inner_name = match ident {
            Some(ident) => Some(self.declare_class_name(ident, class)?),
            None => None,
        };

        // [superclass?, constructor] -> MakeClass -> [class]
        let derived = class.super_class.is_some();
        if let Some(super_class) = &class.super_class {
            self.compile_expr(super_class)?;
        }
        // the body's private names are fresh each time the class is evaluated; the
        // superclass expression above still sees the outer ones
        self.declare_private_names(class)?;
        self.compile_constructor(class, name, derived)?;
        self.emit_op(Operation::MakeClass(derived));

        // methods and accessors now; fields' keys now and values on construction
        let mut statics = Vec::new();
        let mut instance_fields = Vec::new();
        for member in &class.body {
            self.compile_class_member(member, class, &mut statics, &mut instance_fields)?;
        }
        // all instance field values are set up by one hidden method, run on each new instance
        if !instance_fields.is_empty() {
            self.emit_op(Operation::Dup);
            self.compile_hidden_method(&FnBody::InstanceFields(&instance_fields))?;
            self.emit_op(Operation::ClassInit);
        }

        // the class's own name is usable from here on (static initialisers can use it)
        if let Some(slot) = inner_name {
            self.emit_store_slot(slot);
        }
        for init in statics {
            self.compile_static_init(init)?;
        }
        self.exit_scope();
        Ok(())
    }

    /// The class's own name inside its body. `class A extends A` can't use it; methods
    /// (made later) always can.
    fn declare_class_name(&mut self, ident: &Ident, class: &Class) -> CompileResult<u8> {
        let body_start = class
            .super_class
            .as_ref()
            .map_or(ident.span.hi, |s| s.span().hi);
        let slot = self.declare_with_tdz(ident.sym.to_string(), body_start)?;
        self.last_local().read_only = true;
        Ok(slot)
    }

    /// A hidden local per private name, holding the VM's private name object.
    fn declare_private_names(&mut self, class: &Class) -> CompileResult {
        for (key, kind) in class_private_names(class)? {
            let slot = self.declare_variable(key)?;
            self.last_local().private = Some(kind);
            self.emit_op(Operation::NewPrivateName(kind));
            self.emit_store_slot(slot);
            self.emit_op(Operation::Pop);
        }
        Ok(())
    }

    /// Push the constructor: the written one, or an implicit one.
    fn compile_constructor(&mut self, class: &Class, name: Option<&str>, derived: bool) -> CompileResult {
        let kind = if derived {
            FnKind::DerivedClass
        } else {
            FnKind::BaseClass
        };
        let options = FnOptions {
            name,
            ..FnOptions::new(kind)
        };
        let constructor = class.body.iter().find_map(|m| match m {
            ClassMember::Constructor(c) => Some(c),
            _ => None,
        });
        let Some(constructor) = constructor else {
            // `constructor(...args) { super(...args); }` for derived classes
            let body = if derived {
                FnBody::DefaultDerived
            } else {
                FnBody::Block(&[])
            };
            return self.compile_closure(&[], &body, options);
        };
        let mut params = Vec::new();
        for param in &constructor.params {
            match param {
                ParamOrTsParamProp::Param(param) => params.push(&param.pat),
                ParamOrTsParamProp::TsParamProp(p) => return unsupported("parameter property", p.span),
            }
        }
        let body = match &constructor.body {
            Some(body) => FnBody::Block(&body.stmts),
            None => FnBody::Block(&[]),
        };
        self.compile_closure(&params, &body, options)
    }

    /// [class] -> [class]: define a method or accessor now, or collect a field / static
    /// block for later.
    fn compile_class_member<'c>(
        &mut self,
        member: &'c ClassMember,
        class: &Class,
        statics: &mut Vec<StaticInit<'c>>,
        instance_fields: &mut Vec<FieldInit<'c>>,
    ) -> CompileResult {
        match member {
            ClassMember::Constructor(_) | ClassMember::Empty(_) => {}
            ClassMember::Method(method) => {
                // [class] -> [class, target]
                self.emit_op(Operation::Dup);
                if !method.is_static {
                    self.emit_op(load_str("prototype"));
                    self.emit_op(Operation::GetProperty);
                }
                let flavor = method_flavor(method.kind);
                self.compile_method(&method.key, &method.function, flavor)?;
                self.emit_op(Operation::DefineMethod(flavor));
            }
            ClassMember::ClassProp(prop) if prop.is_static => {
                // a computed key is evaluated (and converted) in source order, its value
                // later with the other static initialisers
                let key_slot = match &prop.key {
                    PropName::Computed(_) => {
                        let slot = self.alloc_slot()?;
                        self.compile_prop_name(&prop.key)?;
                        self.emit_op(Operation::ToPropertyKey);
                        self.emit_set_local_pop(slot);
                        Some(slot)
                    }
                    _ => None,
                };
                statics.push(StaticInit::Field(prop, key_slot));
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
            ClassMember::StaticBlock(block) => statics.push(StaticInit::Block(block)),
            ClassMember::PrivateMethod(method) => {
                // [class] -> [class, class, name, fn] -> [class]
                self.emit_op(Operation::Dup);
                self.emit_private_name(&method.key)?;
                let key = private_key(&method.key);
                let flavor = method_flavor(method.kind);
                let name = match flavor {
                    MethodFlavor::Method => key,
                    MethodFlavor::Getter => format!("get {key}"),
                    MethodFlavor::Setter => format!("set {key}"),
                };
                // accessors are only ever called by the VM; a method only needs a host
                // wrapper if it's used as a value (all its uses are in this class body)
                let internal =
                    flavor != MethodFlavor::Method || !private_method_escapes(&method.key, class);
                let options = FnOptions {
                    name: Some(&name),
                    internal,
                    ..FnOptions::new(FnKind::Method)
                };
                self.compile_fn_value(&method.function, options)?;
                self.emit_op(Operation::DefinePrivateMethod(flavor, method.is_static));
            }
            ClassMember::PrivateProp(prop) if prop.is_static => {
                statics.push(StaticInit::PrivateField(prop))
            }
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
        Ok(())
    }

    /// [class] -> [class]: run a static field or block, with `this` as the class.
    fn compile_static_init(&mut self, init: StaticInit) -> CompileResult {
        self.emit_op(Operation::Dup);
        match init {
            StaticInit::Field(prop, key_slot) => {
                match key_slot {
                    Some(slot) => self.emit_op(Operation::GetLocal(slot)),
                    None => self.compile_prop_name(&prop.key)?,
                }
                self.compile_field_init(static_key_name(&prop.key), prop.value.as_deref())?;
                self.emit_op(Operation::StaticField);
            }
            StaticInit::PrivateField(prop) => {
                self.emit_private_name(&prop.key)?;
                self.compile_field_init(Some(private_key(&prop.key)), prop.value.as_deref())?;
                self.emit_op(Operation::StaticField);
            }
            StaticInit::Block(block) => {
                self.compile_hidden_method(&FnBody::Block(&block.body.stmts))?;
                self.emit_op(Operation::StaticBlock);
            }
        }
        Ok(())
    }

    /// Push a field's initialiser: a hidden method returning the value (run with `this` as
    /// the instance, or the class for static fields), or `undefined` for `x;`. `name` names
    /// an anonymous function value.
    fn compile_field_init(&mut self, name: Option<String>, value: Option<&Expr>) -> CompileResult {
        match value {
            Some(value) => self.compile_hidden_method(&FnBody::Field(value, name)),
            None => {
                self.emit_op(Operation::Undefined);
                Ok(())
            }
        }
    }

    /// Push the key of `super.x` / `super[x]`.
    pub(super) fn compile_super_prop(&mut self, prop: &SuperProp) -> CompileResult {
        match prop {
            SuperProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            SuperProp::Computed(computed) => self.compile_expr(&computed.expr)?,
        }
        Ok(())
    }
}

/// A static member, run in order once the class exists.
enum StaticInit<'c> {
    /// a static field, with its computed key's slot (already evaluated)
    Field(&'c ClassProp, Option<u8>),
    PrivateField(&'c PrivateProp),
    Block(&'c StaticBlock),
}

fn method_flavor(kind: MethodKind) -> MethodFlavor {
    match kind {
        MethodKind::Method => MethodFlavor::Method,
        MethodKind::Getter => MethodFlavor::Getter,
        MethodKind::Setter => MethodFlavor::Setter,
    }
}
