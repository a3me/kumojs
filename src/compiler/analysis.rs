//! Static analysis over the AST: `var` collection, escape and capture scans, and small
//! AST helpers.

use super::prelude::*;

/// Collects `var` names declared anywhere in a function body, without entering nested functions.
pub(super) struct VarCollector<'n> {
    pub(super) names: &'n mut Vec<String>,
}

impl Visit for VarCollector<'_> {
    fn visit_var_decl(&mut self, n: &VarDecl) {
        if n.kind == VarDeclKind::Var {
            for decl in &n.decls {
                for ident in pat_idents(&decl.name) {
                    self.names.push(ident.sym.to_string());
                }
            }
        }
        n.visit_children_with(self);
    }
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Whether the function named `name` could be seen by anything other than a direct call
/// `name(...)` within `stmts`. Only directly-called functions can skip the host wrapper:
/// any other read (`g = f`, `arr.map(f)`, `window.x = f`, `typeof f`, `new f`, `{ f }`)
/// lets the value leave VM control. Shadowed names in nested scopes are counted too, which
/// can only make the answer more cautious.
pub(super) fn fn_escapes(name: &str, stmts: &[&Stmt]) -> bool {
    let mut scan = EscapeScan {
        name,
        escapes: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.escapes
}

pub(super) fn fn_escapes_in_function(name: &str, function: &Function) -> bool {
    let mut scan = EscapeScan {
        name,
        escapes: false,
    };
    function.visit_with(&mut scan);
    scan.escapes
}

struct EscapeScan<'n> {
    name: &'n str,
    escapes: bool,
}

impl EscapeScan<'_> {
    fn is_name(&self, expr: &Expr) -> bool {
        matches!(expr.unwrap_parens(), Expr::Ident(ident) if *ident.sym == *self.name)
    }
}

impl Visit for EscapeScan<'_> {
    fn visit_call_expr(&mut self, n: &CallExpr) {
        match &n.callee {
            // a direct call doesn't let the function out; its arguments still might
            Callee::Expr(callee) if self.is_name(callee) => n.args.visit_with(self),
            _ => n.visit_children_with(self),
        }
    }
    fn visit_expr(&mut self, n: &Expr) {
        if self.is_name(n) {
            self.escapes = true;
        }
        n.visit_children_with(self);
    }
    fn visit_prop(&mut self, n: &Prop) {
        if let Prop::Shorthand(ident) = n {
            if *ident.sym == *self.name {
                self.escapes = true;
            }
        }
        n.visit_children_with(self);
    }
}

/// Names used anywhere inside functions nested in `stmts` (not counting `stmts`' own code).
/// A local with one of these names may be captured, so it's kept in a box. Matching by
/// name alone ignores shadowing, which can only box more variables than needed.
/// `params` count too: a default value can hold a closure.
pub(super) fn captured_names(params: &[&Pat], body: &FnBody, stmts: &[&Stmt]) -> HashSet<String> {
    let mut scan = CaptureScan {
        names: HashSet::new(),
        fn_depth: 0,
    };
    for param in params {
        param.visit_with(&mut scan);
    }
    if let FnBody::Expr(expr) | FnBody::Field(expr, _) = body {
        expr.visit_with(&mut scan);
    }
    if let FnBody::InstanceFields(fields) = body {
        for value in fields.iter().filter_map(|f| f.value) {
            value.visit_with(&mut scan);
        }
    }
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.names
}

struct CaptureScan {
    names: HashSet<String>,
    /// how many nested functions deep the visitor is
    fn_depth: usize,
}

impl Visit for CaptureScan {
    fn visit_function(&mut self, n: &Function) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_arrow_expr(&mut self, n: &ArrowExpr) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_constructor(&mut self, n: &Constructor) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_static_block(&mut self, n: &StaticBlock) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    // a field's key is evaluated in the enclosing scope, its value in its own function
    fn visit_class_prop(&mut self, n: &ClassProp) {
        n.key.visit_with(self);
        self.fn_depth += 1;
        n.value.visit_with(self);
        self.fn_depth -= 1;
    }
    fn visit_private_prop(&mut self, n: &PrivateProp) {
        self.fn_depth += 1;
        n.value.visit_with(self);
        self.fn_depth -= 1;
    }
    // private names live in hidden locals named `#x`
    fn visit_private_name(&mut self, n: &PrivateName) {
        if self.fn_depth > 0 {
            self.names.insert(private_key(n));
        }
    }
    fn visit_ident(&mut self, n: &Ident) {
        if self.fn_depth > 0 {
            self.names.insert(n.sym.to_string());
        }
    }
}

/// The hidden local name for a private name: `#x` (can't clash with an identifier).
pub(super) fn private_key(name: &PrivateName) -> String {
    format!("#{}", name.name)
}

/// The private names a class body declares, in source order, with what each one is.
/// A getter and setter pair share one name.
pub(super) fn class_private_names(class: &Class) -> CompileResult<Vec<(String, PrivateKind)>> {
    let mut names: Vec<(String, PrivateKind, bool)> = Vec::new();
    for member in &class.body {
        let (name, kind, is_static) = match member {
            ClassMember::PrivateProp(p) => (&p.key, PrivateKind::Field, p.is_static),
            ClassMember::PrivateMethod(m) if m.kind == MethodKind::Method => {
                (&m.key, PrivateKind::Method, m.is_static)
            }
            ClassMember::PrivateMethod(m) => (&m.key, PrivateKind::Accessor, m.is_static),
            _ => continue,
        };
        let key = private_key(name);
        match names.iter().find(|(k, ..)| *k == key) {
            None => names.push((key, kind, is_static)),
            // the other half of an accessor pair
            Some((_, PrivateKind::Accessor, s))
                if kind == PrivateKind::Accessor && *s == is_static => {}
            Some(_) => return unsupported(format!("duplicate private name `{key}`"), name.span),
        }
    }
    Ok(names
        .into_iter()
        .map(|(key, kind, _)| (key, kind))
        .collect())
}

/// Whether private method `name` is used other than by calling it (`x.#m(...)`) in the
/// class body. Private names can only be used inside the class body, so this is exact
/// (an inner class reusing the name only makes it more cautious).
pub(super) fn private_method_escapes(name: &PrivateName, class: &Class) -> bool {
    let mut scan = PrivateEscapeScan {
        name: &name.name,
        escapes: false,
    };
    class.visit_with(&mut scan);
    scan.escapes
}

struct PrivateEscapeScan<'n> {
    name: &'n str,
    escapes: bool,
}

impl PrivateEscapeScan<'_> {
    fn is_name(&self, prop: &MemberProp) -> bool {
        matches!(prop, MemberProp::PrivateName(p) if *p.name == *self.name)
    }
}

impl Visit for PrivateEscapeScan<'_> {
    fn visit_call_expr(&mut self, n: &CallExpr) {
        let Callee::Expr(callee) = &n.callee else {
            return n.visit_children_with(self);
        };
        match callee.unwrap_parens() {
            // a method call doesn't let the function out
            Expr::Member(member) if self.is_name(&member.prop) => {
                member.obj.visit_with(self);
                n.args.visit_with(self);
            }
            _ => n.visit_children_with(self),
        }
    }
    fn visit_member_expr(&mut self, n: &MemberExpr) {
        if self.is_name(&n.prop) {
            self.escapes = true;
        }
        n.visit_children_with(self);
    }
}

/// The names a binding pattern declares, in source order.
pub(super) fn pat_idents(pat: &Pat) -> Vec<&Ident> {
    fn walk<'p>(pat: &'p Pat, out: &mut Vec<&'p Ident>) {
        match pat {
            Pat::Ident(ident) => out.push(&ident.id),
            Pat::Array(array_pat) => {
                for elem in array_pat.elems.iter().flatten() {
                    walk(elem, out);
                }
            }
            Pat::Object(object_pat) => {
                for prop in &object_pat.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => walk(&key_value.value, out),
                        ObjectPatProp::Assign(assign) => out.push(&assign.key.id),
                        ObjectPatProp::Rest(rest) => walk(&rest.arg, out),
                    }
                }
            }
            Pat::Assign(assign) => walk(&assign.left, out),
            Pat::Rest(rest) => walk(&rest.arg, out),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }
    let mut out = Vec::new();
    walk(pat, &mut out);
    out
}

/// A property key known at compile time, as a function name (`None` for computed keys and
/// numbers that need JS formatting).
pub(super) fn static_key_name(key: &PropName) -> Option<String> {
    match key {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(s) => Some(String::from_utf8_lossy(s.value.as_bytes()).into_owned()),
        PropName::Num(n) if n.value.fract() == 0.0 && n.value < 1e21 => {
            Some(format!("{}", n.value as u64))
        }
        _ => None,
    }
}

/// A non-computed `__proto__` key.
pub(super) fn is_proto_key(key: &PropName) -> bool {
    match key {
        PropName::Ident(ident) => &*ident.sym == "__proto__",
        PropName::Str(s) => s.value.as_bytes() == b"__proto__",
        _ => false,
    }
}

/// An unnamed function or arrow expression, which takes its name from where it's stored.
pub(super) fn is_anonymous_fn(expr: &Expr) -> bool {
    match expr.unwrap_parens() {
        Expr::Fn(f) => f.ident.is_none(),
        Expr::Class(c) => c.ident.is_none(),
        Expr::Arrow(_) => true,
        _ => false,
    }
}
