let log = window.console.log;
let json = window.JSON.stringify;

let counter = 0;
class Fields {
    a = 1;
    b = this.a + 1;
    c;
    ["comp" + "uted"] = "yes";
    id = ++counter;
    arrow = () => this.a;
    static count = 10;
    static double = Fields.count * 2;
    static {
        this.fromBlock = this.count + 1;
    }
}
let f1 = new Fields(), f2 = new Fields();
log(json(f1), f2.id, f1.arrow.call(null), f1.arrow.name);
log(Fields.count, Fields.double, Fields.fromBlock);
log(window.Object.getOwnPropertyDescriptor(f1, "a").enumerable, "c" in f1, f1.c);

// fields are set before the constructor body runs, and after super() in subclasses
class Base {
    baseField = "base";
    constructor() { this.seenInBase = this.subField; }
}
class Sub extends Base {
    subField = "sub";
    constructor() {
        super();
        this.seenInSub = this.subField;
    }
}
let s = new Sub();
log(s.baseField, s.seenInBase, s.seenInSub);
class SubDefault extends Base { extra = this.baseField + "!"; }
log(new SubDefault().extra);

// fields shadow setters on the prototype (defined, not assigned)
class WithSetter { set x(v) { this.setterRan = true; } }
class Shadow extends WithSetter { x = 5; }
let sh = new Shadow();
log(sh.x, sh.setterRan);

// closures and `super` in field initialisers
class Parent { greet() { return "hi"; } }
class Child extends Parent {
    greeting = super.greet() + "!";
    fn = function () { return "plain"; };
}
let c = new Child();
log(c.greeting, c.fn(), c.fn.name);
let made = [];
for (let i = 0; i < 2; i++) made.push(new (class { v = i; })().v);
log(made.join(","));
