let log = window.console.log;
let json = window.JSON.stringify;

// shorthand and computed keys
let a = 1, b = 2, k = "dyn";
let o = { a, b, [k]: 3, [k + "2"]: 4, ["x" + 1]: 5, [1 + 1]: "two" };
log(json(o));

// methods
let counter = {
    count: 0,
    inc() { this.count++; return this; },
    "quoted name"() { return "q"; },
    [k + "Method"]() { return "computed"; },
};
log(counter.inc().inc().count, counter["quoted name"](), counter.dynMethod());
log(counter.inc.name, counter["quoted name"].name, counter.dynMethod.name);
try { new counter.inc(); } catch (e) { log(e instanceof window.TypeError); }
log(counter.inc.prototype, typeof counter.inc);

// getters and setters
let temp = {
    c: 20,
    get f() { return this.c * 9 / 5 + 32; },
    set f(v) { this.c = (v - 32) * 5 / 9; },
    get [k]() { return "computed getter"; },
};
log(temp.f);
temp.f = 212;
log(temp.c, temp.dyn);
let desc = window.Object.getOwnPropertyDescriptor(temp, "f");
log(typeof desc.get, typeof desc.set, desc.enumerable, desc.configurable, desc.get.name, desc.set.name);
log(window.Object.getOwnPropertyDescriptor(temp, "dyn").get.name);
let onlyGet = { get x() { return 1; } };
onlyGet.x = 5;
log(onlyGet.x);
// later definitions replace earlier ones
log(json({ get v() { return 1; }, v: 2 }), json({ v: 1, get v() { return 3; } }));

// spread
let base = { p: 1, q: 2 };
let sym = window.Symbol("s");
let withSym = { [sym]: "symval", r: 3 };
let spread = { ...base, q: 20, ...withSym, ...null, ...undefined, ..."hi", ...[9] };
log(json(spread), spread[sym]);
let getterSrc = { get g() { return "evaluated"; } };
let copied = { ...getterSrc };
log(typeof window.Object.getOwnPropertyDescriptor(copied, "g").get, copied.g);
let protoSrc = window.Object.create({ inherited: 1 });
protoSrc.own = 1;
log(json({ ...protoSrc }));

// __proto__ sets the prototype
let parent = { greet() { return "from parent"; } };
let child = { __proto__: parent, own: 1 };
log(child.greet(), window.Object.getPrototypeOf(child) === parent, json(child));
let noProto = { __proto__: null, get z() { return 1; } };
log(window.Object.getPrototypeOf(noProto));

// names inferred from where functions are stored
let fnExpr = function () {};
let arrow = () => {};
let paren = (function () {});
let named = function inner() {};
let assigned;
assigned = () => {};
let props = { m: function () {}, n: () => {}, [k]: () => {}, [sym]: function () {}, 7: () => {} };
log(fnExpr.name, arrow.name, paren.name, named.name, assigned.name);
log(props.m.name, props.n.name, props.dyn.name, props[sym].name, props[7].name);
let { d = () => {} } = {};
let [e = function () {}] = [];
function param(p = () => {}) { return p.name; }
log(d.name, e.name, param());
