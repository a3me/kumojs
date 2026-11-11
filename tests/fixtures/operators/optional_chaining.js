let log = window.console.log;

let n = null;
let u;
let o = {
    a: { b: { c: 1 }, list: [10, 20], f() { return this === o.a; } },
    nil: null,
    count: 0,
    inc() { return ++this.count; },
};

// members, computed members, deep chains
log(n?.a, u?.a.b.c, o?.a.b.c, o.a?.b?.c, o.nil?.x.y.z, o.a?.list?.[1], n?.[log("not evaluated")]);

// calls keep `this`, and short-circuit the arguments too
let calls = 0;
log(o.a?.f(), o.a.f?.(), o?.a.f(), o.missing?.(calls++), n?.f(calls++), calls);
log(o.inc?.(), o?.inc(), o.count);
let fn = null;
log(fn?.(), u?.(), (() => 7)?.());

// a call result continues the chain
let mk = () => ({ v: { w: "deep" } });
log(mk?.().v?.w, mk().v?.w.length);

// parentheses end the chain
let errName = (f) => {
    try {
        f();
        return "no error";
    } catch (e) {
        return e.name;
    }
};
log(errName(() => (n?.a).b), errName(() => n?.a.b), errName(() => o.nil.x?.y));

// falsy but non-nullish values don't short-circuit
let z = { zero: 0, empty: "", f: false };
log(z.zero?.toFixed(1), z.empty?.length, z.f?.toString());

// delete through a chain
let d = { inner: { x: 1 } };
log(delete d?.inner.x, d.inner.x, delete n?.x, delete d.missing?.x);

// private names and super in chains
class P {
    #x = 1;
    static read(obj) {
        return obj?.#x;
    }
    hi() {
        return "hi";
    }
}
class C extends P {
    test() {
        return [super.hi?.(), super.nope?.()].join(",");
    }
}
log(P.read(new P()), P.read(null), new C().test());

// with other operators
log(n?.a ?? "default", (o.a?.b.c ?? 0) + 1, typeof n?.a, !o?.a);

// a parenthesised chain called directly keeps `this`
log((o.a?.f)(), (o?.a.f)(), errName(() => (n?.f)()));
