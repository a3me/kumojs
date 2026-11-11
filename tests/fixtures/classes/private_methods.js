let log = window.console.log;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

class Temp {
    #c = 0;
    #toF(c) {
        return c * 9 / 5 + 32;
    }
    get #f() {
        return this.#toF(this.#c);
    }
    set #f(v) {
        this.#c = (v - 32) * 5 / 9;
    }
    get #readOnly() {
        return "ro";
    }
    set #writeOnly(v) {
        this.last = v;
    }
    setF(v) {
        // assignment evaluates to the assigned value, not the setter's result
        return (this.#f = v);
    }
    getF() {
        return this.#f;
    }
    bump() {
        this.#f += 18;
        return this.#f++;
    }
    misc() {
        return [this.#readOnly, (this.#writeOnly = "w"), this.last].join(",");
    }
    badWrites() {
        return [
            errName(() => { this.#toF = 1; }),
            errName(() => { this.#readOnly = 1; }),
            errName(() => this.#writeOnly),
        ].join(",");
    }
}
let t = new Temp();
log(t.setF(212), t.getF(), t.bump(), t.getF(), t.misc(), t.badWrites());

// methods are shared between instances, have names, and keep `this` when called
class M {
    #m() {
        return this;
    }
    #arrowUser() {
        return [1, 2].map((x) => this.#m() === this && x).join(",");
    }
    get(o) {
        return o.#m;
    }
    call(o) {
        return o.#m() === o;
    }
    arrows() {
        return this.#arrowUser();
    }
}
let m1 = new M(), m2 = new M();
log(m1.get(m1) === m2.get(m2), m1.get(m1).name, m1.call(m2), m1.arrows());
log(errName(() => m1.get({})), errName(() => m1.call({})));
// an escaped private method can be called with any `this`
let escaped = m1.get(m1);
log(escaped.call(5), escaped.length);

// private methods see `super`
class P {
    greet() { return "hi"; }
}
class C extends P {
    #greet() { return super.greet() + "!"; }
    greet() { return this.#greet(); }
}
log(new C().greet());

// methods are installed before field initialisers run
class Order {
    early = this.#value();
    tryLate = (() => {
        try {
            return this.#late;
        } catch (e) {
            return e.name;
        }
    })();
    #late = "late";
    #value() { return "method"; }
}
let o = new Order();
log(o.early, o.tryLate);

// the brand check also guards accessors
class Acc {
    get #v() { return 1; }
    static read(o) { return o.#v; }
}
log(Acc.read(new Acc()), errName(() => Acc.read({})));

// a parent constructor returning an object gets the subclass's methods' brand
class Ret {
    constructor(o) { return o; }
}
class Branded extends Ret {
    #hello() { return "hello"; }
    static hello(o) { return o.#hello(); }
}
let obj = {};
new Branded(obj);
log(Branded.hello(obj), errName(() => new Branded(obj)));

// private methods in static and instance context together with callbacks
class Sorter {
    #key(x) { return -x; }
    sort(arr) {
        return arr.slice().sort((a, b) => this.#key(a) - this.#key(b)).join(",");
    }
}
log(new Sorter().sort([3, 1, 2]));
