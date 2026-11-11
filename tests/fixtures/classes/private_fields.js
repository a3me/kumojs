let log = window.console.log;
let json = window.JSON.stringify;
let Reflect = window.Reflect;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

class Counter {
    #count = 0;
    #step;
    pub = 1;
    #label = "c" + this.pub; // initialisers run in source order with public fields
    constructor(step) {
        this.#step = step;
    }
    inc() {
        this.#count += this.#step;
        return this.#count;
    }
    get label() {
        return this.#label;
    }
    static read(c) {
        return c.#count;
    }
}
let c = new Counter(2);
log(c.inc(), c.inc(), Counter.read(c), c.label);

// private fields don't show up as properties
log(json(c), Reflect.ownKeys(c).length, window.Object.keys(c).join(","), "#count" in c);

// updates, compound and logical assignment
class Ops {
    #n = 5;
    #s = null;
    run() {
        let out = [];
        out.push(this.#n++, this.#n, ++this.#n, this.#n--, --this.#n);
        this.#n *= 3;
        this.#n -= 1;
        this.#n **= 2;
        out.push(this.#n);
        this.#s ??= "set";
        this.#s ||= "no";
        this.#s &&= this.#s + "!";
        out.push(this.#s);
        let other = this;
        [this.#n, other.#s] = [1, "two"];
        ({ a: this.#n } = { a: this.#n + 10 });
        out.push(this.#n, this.#s);
        return out.join(" ");
    }
}
log(new Ops().run());

// each instance (and other instances of the same class) can be read
class Pt {
    #x;
    constructor(x) { this.#x = x; }
    equals(o) { return this.#x === o.#x; }
}
log(new Pt(1).equals(new Pt(1)), new Pt(1).equals(new Pt(2)));

// reading from an object without the field throws a TypeError
log(errName(() => Counter.read({})), errName(() => Counter.read(null)), errName(() => Counter.read(5)));
log(errName(() => new Pt(1).equals({})));

// writing to an object without the field throws too (and doesn't add it)
class Setter {
    #v = 1;
    static put(o, v) { o.#v = v; return o.#v; }
}
let plain = {};
log(errName(() => Setter.put(plain, 2)), Setter.put(new Setter(), 7));

// subclass instances get the parent's private fields; a subclass's own privates are separate
class Base {
    #secret = "base";
    baseSecret() { return this.#secret; }
}
class Derived extends Base {
    #secret = "derived";
    derivedSecret() { return this.#secret; }
}
let d = new Derived();
log(d.baseSecret(), d.derivedSecret());

// privates are added to whatever object a parent constructor returns
class ReturnsOther {
    constructor(o) { return o; }
}
class Stamp extends ReturnsOther {
    #tag = "stamped";
    static tag(o) { return o.#tag; }
    static has(o) { return #tag in o; }
}
let target = { plain: true };
new Stamp(target);
log(Stamp.tag(target), json(target), Stamp.has(target), Stamp.has({}));
// stamping the same object twice throws
log(errName(() => new Stamp(target)));

// frozen objects can still get private fields
let frozen = window.Object.freeze({});
new Stamp(frozen);
log(Stamp.tag(frozen));

// arrow functions and closures in initialisers see `this`'s privates
class Closures {
    #v = 3;
    getV = () => this.#v;
    #f = function () { return "named"; };
    fname() { return this.#f.name; }
    inner() {
        let read = (o) => o.#v;
        return [1, 2].map(() => read(this)).join("");
    }
}
let cl = new Closures();
log(cl.getV(), cl.getV.call(null), cl.fname(), cl.inner());

// every evaluation of a class body makes new private names
let makers = [];
for (let i = 0; i < 2; i++) {
    makers.push(class {
        #id = i;
        static id(o) { return o.#id; }
    });
}
let a0 = new makers[0]();
log(makers[0].id(a0), errName(() => makers[1].id(a0)));

// nested classes can use the outer class's privates; inner names shadow
class Outer {
    #x = "outer";
    #y = "outer-y";
    test() {
        let self = this;
        class Inner {
            #x = "inner";
            read() { return this.#x + "/" + self.#y; }
            // `#x` here is Inner's, which `self` doesn't have
            readOuter() { return self.#x; }
        }
        let inner = new Inner();
        return inner.read() + " " + errName(() => inner.readOuter()) + " " + this.#x;
    }
}
log(new Outer().test());
