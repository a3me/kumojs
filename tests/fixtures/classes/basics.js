let log = window.console.log;
let json = window.JSON.stringify;
let attempt = f => { try { f(); log("no error"); } catch (e) { log(e.name); } };

class Point {
    constructor(x, y) { this.x = x; this.y = y; }
    norm() { return window.Math.sqrt(this.x * this.x + this.y * this.y); }
    get sum() { return this.x + this.y; }
    set sum(v) { this.x = v; this.y = 0; }
    static origin() { return new Point(0, 0); }
    toString() { return `(${this.x}, ${this.y})`; }
    ["comp" + "uted"]() { return "computed"; }
}
let p = new Point(3, 4);
log(p.norm(), p.sum, `${p}`, Point.origin().x, p.computed(), json(p));
p.sum = 10;
log(p.x, p.y);

// shape of a class
log(typeof Point, Point.name, Point.length, p instanceof Point, p.constructor === Point);
log(window.Object.keys(Point.prototype).length, json(window.Object.getOwnPropertyNames(Point.prototype)));
let protoDesc = window.Object.getOwnPropertyDescriptor(Point, "prototype");
log(protoDesc.writable, protoDesc.enumerable, protoDesc.configurable);
let sumDesc = window.Object.getOwnPropertyDescriptor(Point.prototype, "sum");
log(typeof sumDesc.get, typeof sumDesc.set, sumDesc.enumerable, sumDesc.get.name);
log(Point.prototype.norm.name, Point.origin.name, Point.prototype.computed.name);
attempt(() => Point(1, 2));
attempt(() => new Point.prototype.norm());
attempt(() => Point.call({}, 1, 2));

// classes as values: expressions, inferred and inner names
let Anon = class {};
const Named = class Inner { who() { return Inner.name; } };
log(Anon.name, Named.name, new Named().who(), (class {}).name);
let { Dflt = class {} } = {};
log(Dflt.name, ({ k: class {} }).k.name);
class Self { static make() { return new Self(); } }
let Saved = Self;
log(Saved.make() instanceof Saved);

// default constructor, no constructor at all
class Empty {}
log(json(new Empty()), Empty.length);

// returning an object from a constructor
class Factory { constructor() { return { made: true }; } }
log(json(new Factory()), new Factory() instanceof Factory);
class PrimReturn { constructor() { this.a = 1; return 5; } }
log(json(new PrimReturn()));

// classes in loops close over per-iteration bindings
let made = [];
for (let i = 0; i < 2; i++) made.push(class { get i() { return i; } });
log(made.map(C => new C().i).join(","));

// host code can construct and call methods
log(window.Reflect.construct(Point, [1, 1]).sum, [new Point(1, 2), new Point(3, 4)].map(q => q.sum).join(","));

// new.target
function Plain() { return new.target === Plain; }
log(new Plain() instanceof Plain, Plain());
class Target { constructor() { this.t = new.target === Target; this.arrow = (() => new.target)() === Target; } }
log(json(new Target()));
