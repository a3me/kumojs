let log = window.console.log;

// expression and block bodies
let add = (a, b) => a + b;
let sq = x => x * x;
let obj = () => ({ k: 1 });
let block = (x) => { let y = x + 1; return y * 2; };
log(add(2, 3), sq(4), obj().k, block(1), (() => {})());
log(typeof add, add.length, (() => 0).name, [1, 2, 3].map(x => x * 10).join(","));

// lexical `this`
let counter = {
    count: 0,
    start: function () {
        [1, 2, 3].forEach(n => { this.count += n; });
        return this.count;
    },
    nested: function () {
        return (() => (() => this.count)())();
    },
};
log(counter.start(), counter.nested());
let detached = counter.nested;
log(detached === counter.nested);

// `this` stays lexical with call/apply/bind
function Holder() {
    this.v = "holder";
    this.get = () => this.v;
}
let h = new Holder();
log(h.get(), h.get.call({ v: "other" }), h.get.apply({ v: "x" }), h.get.bind({ v: "y" })());
function plain() { return (() => this)(); }
log(plain(), plain.call("bound"));

// lexical `arguments`
function outer() {
    let f = () => arguments.length + ":" + arguments[0];
    return f("ignored", "too");
}
log(outer("a", "b", "c"));

// arrows are not constructors and have no prototype
let Arrow = () => {};
try { new Arrow(); } catch (e) { log(e instanceof window.TypeError); }
log(Arrow.prototype, window.Object.prototype.hasOwnProperty.call(Arrow, "prototype"));

// closures, recursion through a binding, immediately invoked arrows
let makeAdder = n => m => n + m;
log(makeAdder(3)(4));
const fact = n => n <= 1 ? 1 : n * fact(n - 1);
log(fact(6), (x => x * 2)(21));

// arrows in loops capture each iteration
let fns = [];
for (let i = 0; i < 3; i++) fns.push(() => i);
log(fns.map(f => f()).join(","));

// natives calling arrows, sort comparator
log([3, 1, 2].sort((a, b) => b - a).join(","), [1, 2, 3, 4].filter(x => x % 2).join(","));
