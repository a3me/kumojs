let log = window.console.log;
function Point(x, y) {
    this.x = x;
    this.y = y;
}
Point.prototype.sum = function () { return this.x + this.y; };
let p = new Point(2, 3);
log(p.sum(), p instanceof Point, p.constructor === Point, Object_keys(p));
function Object_keys(o) { return window.Object.keys(o).join(","); }

function Returns() { this.a = 1; return { b: 2 }; }
log(new Returns().b, new Returns().a);
function ReturnsPrimitive() { this.a = 1; return 5; }
log(new ReturnsPrimitive().a);

function Animal(name) { this.name = name; }
Animal.prototype.speak = function () { return this.name + " makes a sound"; };
function Dog(name) { Animal.call(this, name); }
Dog.prototype = window.Object.create(Animal.prototype);
Dog.prototype.speak = function () { return this.name + " barks"; };
let d = new Dog("rex");
log(d.speak(), d instanceof Dog, d instanceof Animal);

let date = new window.Date(0);
log(date.getTime(), new window.Array(3).length, new Point(1, 1) instanceof window.Object);
log(typeof new Point(0, 0), typeof Point);

// constructors run directly by the VM: return values, prototypes, exceptions
function RetObj() { this.a = 1; return { b: 2 }; }
function RetPrim() { this.a = 1; return 5; }
function RetNull() { this.a = 1; return null; }
function RetFn() { return function () { return "fn"; }; }
log(window.JSON.stringify(new RetObj()), window.JSON.stringify(new RetPrim()), window.JSON.stringify(new RetNull()), new RetFn()());
function NoProto() { this.v = 1; }
NoProto.prototype = 5;
let np = new NoProto();
log(window.Object.getPrototypeOf(np) === window.Object.prototype, np.v);
function Throws() { throw new window.Error("ctor"); }
try { new Throws(); } catch (e) { log("caught", e.message); }
function Outer() { this.inner = new Inner(2); }
function Inner(x) { this.x = x * 10; }
let o = new Outer();
log(o.inner.x, o instanceof Outer, o.inner instanceof Inner, o.constructor === Outer);
