let log = window.console.log;
let R = window.Reflect;
let order = [];

// fields are set up before parameter defaults run, and after super() returns
class Base {
    a = order.push("Base field");
    constructor(x = order.push("Base param")) {
        order.push("Base body");
    }
}
class Mid extends Base {
    b = order.push("Mid field");
    constructor(y = order.push("Mid param")) {
        order.push("Mid before super");
        super();
        order.push("Mid after super");
    }
}
class Leaf extends Mid {
    c = order.push("Leaf field");
}
new Leaf();
log(order.join(", "));

// field values: named functions, computed keys, new.target, this and super
let k = "dyn";
let sym = window.Symbol("s");
class Fields {
    named = function () {};
    [k] = () => {};
    [sym] = function () {};
    nt = new.target;
    self = this;
    up = super.toString === window.Object.prototype.toString;
}
let f = new Fields();
log(f.named.name, f.dyn.name, f[sym].name, f.nt, f.self === f, f.up);

// extending natives and plain functions, with fields
class MyArray extends window.Array {
    tag = "arr";
}
let ma = new MyArray(1, 2, 3);
log(ma.length, ma.tag, ma instanceof MyArray, window.Array.isArray(ma));
class MyError extends window.Error {
    code = 42;
}
let me = new MyError("msg");
log(me.message, me.code, me instanceof window.Error);
function Plain(v) {
    this.v = v;
}
class FromPlain extends Plain {
    w = this.v + 1;
}
let fp = new FromPlain(1);
log(fp.v, fp.w);

// Reflect.construct with another new.target, and calling from the host
class Target {
    t = 1;
    constructor() {
        this.proto = window.Object.getPrototypeOf(this) === Other.prototype;
    }
}
function Other() {}
let viaReflect = R.construct(Target, [], Other);
log(viaReflect.t, viaReflect.proto);
let made = [1, 2].map((x) => new Leaf());
log(made.length, made[0].c);

// super() returning another object, super() twice, `this` before super
class Returns {
    constructor() {
        return { replaced: true };
    }
}
class GetsReplaced extends Returns {
    extra = "added";
}
let gr = new GetsReplaced();
log(gr.replaced, gr.extra, gr instanceof GetsReplaced);
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};
class Twice extends Base {
    constructor() {
        super();
        super();
    }
}
class Early extends Base {
    constructor() {
        this.x = 1;
        super();
    }
}
class ThrowsInField {
    bad = (() => {
        throw new window.RangeError("field");
    })();
}
log(errName(() => new Twice()), errName(() => new Early()), errName(() => new ThrowsInField()));

// super() inside an arrow in the constructor
class ArrowSuper extends Base {
    d = "d";
    constructor() {
        let call = () => super();
        call();
    }
}
log(new ArrowSuper().d);
