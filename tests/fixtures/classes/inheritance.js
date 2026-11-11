let log = window.console.log;
let json = window.JSON.stringify;
let attempt = f => { try { f(); log("no error"); } catch (e) { log(e.name); } };

class Animal {
    constructor(name) { this.name = name; }
    speak() { return `${this.name} makes a sound`; }
    get kind() { return "animal"; }
    static create(name) { return new this(name); }
    static describe() { return "Animal class"; }
}
class Dog extends Animal {
    constructor(name, breed) { super(name); this.breed = breed; }
    speak() { return super.speak() + " (woof)"; }
    get kind() { return "dog, a kind of " + super.kind; }
    static describe() { return super.describe() + " > Dog"; }
}
let d = new Dog("Rex", "lab");
log(d.speak(), d.kind, d.breed, Dog.describe());
log(d instanceof Dog, d instanceof Animal, Dog.create("Fido") instanceof Dog, Dog.create("Fido").name);
log(window.Object.getPrototypeOf(Dog) === Animal, window.Object.getPrototypeOf(Dog.prototype) === Animal.prototype);

// default derived constructor passes all arguments
class Puppy extends Dog {}
let pup = new Puppy("Bit", "pug");
log(pup.name, pup.breed, pup.speak(), Puppy.length);

// this before super, super twice, missing super
class Early extends Animal { constructor() { this.x = 1; super("e"); } }
attempt(() => new Early());
class Twice extends Animal { constructor() { super("a"); super("b"); } }
attempt(() => new Twice());
class NoSuper extends Animal { constructor() {} }
attempt(() => new NoSuper());
class ReturnsObject extends Animal { constructor() { return { custom: 1 }; } }
log(json(new ReturnsObject()));
class ArrowSuper extends Animal { constructor() { const go = () => super("arrow"); go(); } }
log(new ArrowSuper().name);

// extending host built-ins
class MyError extends window.Error {
    constructor(message) { super(message); this.name = "MyError"; }
}
let err = new MyError("boom");
log(err instanceof MyError, err instanceof window.Error, err.message, `${err}`, typeof err.stack);
try { throw new MyError("thrown"); } catch (e) { log(e.name, e.message); }
class Stack extends window.Array {
    peek() { return this[this.length - 1]; }
}
let st = new Stack();
st.push(1, 2, 3);
log(st.peek(), st.length, window.Array.isArray(st), st instanceof Stack, st.map(x => x * 2) instanceof Stack);
class Counter extends window.Map {
    inc(k) { this.set(k, (this.get(k) || 0) + 1); return this; }
}
log(new Counter().inc("a").inc("a").get("a"));

// extending a plain function, and null
function OldStyle(v) { this.v = v; }
OldStyle.prototype.get = function () { return this.v; };
class Modern extends OldStyle { constructor() { super(7); } }
log(new Modern().get());
class NullBase extends null {}
log(window.Object.getPrototypeOf(NullBase.prototype));
attempt(() => new NullBase());
attempt(() => { class Bad extends 5 {} });
attempt(() => { const notCtor = () => {}; class Bad extends notCtor {} });

// super property assignment and computed super access
class Base2 { get v() { return "base"; } m() { return "m"; } }
class Sub2 extends Base2 {
    test() {
        super.written = 1;
        return [super["m"](), this.written, super.v].join(",");
    }
}
log(new Sub2().test());

// super in object literal methods
let proto = { hi() { return "proto hi"; } };
let objWithSuper = { __proto__: proto, hi() { return super.hi() + " via literal"; } };
log(objWithSuper.hi());
