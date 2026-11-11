let log = window.console.log;
let O = window.Object;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};
let show = (r) => JSON.stringify(r);
let JSON = window.JSON;

// next() steps through, values sent in come back from `yield`
function* counter(start) {
    let got = [];
    let x = yield start;
    got.push(x);
    x = yield start + 1;
    got.push(x);
    return got.join("+");
}
let g = counter(10);
log(show(g.next("ignored")), show(g.next("a")), show(g.next("b")), show(g.next()), show(g.next()));

// generators in for-of, spread, destructuring, Array.from
function* range(n) {
    for (let i = 0; i < n; i++) yield i;
}
let collected = [];
for (let v of range(3)) collected.push(v);
let [first, , third] = range(5);
log(collected.join(","), [...range(4)].join(","), first, third, window.Array.from(range(2)).join(","));

// parameters are set up when called; the body waits for the first next()
let order = [];
function* lazy(a = order.push("param")) {
    order.push("body");
    yield a;
}
let lz = lazy();
order.push("called");
lz.next();
log(order.join(" "));

// return() runs finally blocks; a finally can yield again
function* withFinally() {
    try {
        yield 1;
        yield 2;
    } finally {
        log("cleanup");
        yield "from finally";
    }
}
let wf = withFinally();
log(show(wf.next()), show(wf.return("early")), show(wf.next()), show(wf.next()));

// throw() is thrown at the yield, and can be caught there
function* catcher() {
    while (true) {
        try {
            yield "waiting";
        } catch (e) {
            log("caught", e);
        }
    }
}
let c = catcher();
c.next();
log(show(c.throw("oops")), show(c.return(5)));

// throw() before starting or after finishing
let fresh = counter(0);
log(errName(() => fresh.throw(new window.TypeError("x"))), show(fresh.next()));
log(show(range(1).return(7)));

// an uncaught throw finishes the generator
function* thrower() {
    yield 1;
    throw new window.RangeError("bad");
}
let t = thrower();
t.next();
log(errName(() => t.next()), show(t.next()));

// yield* passes values, throws and returns through, and gives the inner return value
function* inner() {
    try {
        let a = yield "i1";
        log("inner got", a);
        yield "i2";
    } finally {
        log("inner finally");
    }
    return "inner done";
}
function* outer() {
    let r = yield* inner();
    log("yield* result", r);
    yield* [7, 8];
    yield* "ab";
}
log([...outer()].join(","));
let o = outer();
o.next();
log(show(o.next("sent")), show(o.return("stop")), show(o.next()));

// break out of a for-of over a generator closes it (runs its finally)
for (let v of inner()) {
    log("got", v);
    break;
}

// `this`, `arguments` and closures in generators; generator methods
let obj = {
    base: 100,
    *gen() {
        yield this.base + arguments.length;
        let f = () => this.base * 2;
        yield f();
    },
};
class K {
    constructor(n) {
        this.n = n;
    }
    *[window.Symbol.iterator]() {
        for (let i = 0; i < this.n; i++) yield i * 10;
    }
    static *count() {
        yield "static";
    }
}
log([...obj.gen(1, 2)].join(","), [...new K(3)].join(","), K.count().next().value);

// generator objects, prototypes and protocol details
function* proto() {}
let p = proto();
let GeneratorProto = O.getPrototypeOf(O.getPrototypeOf(p));
log(O.getPrototypeOf(p) === proto.prototype, p instanceof proto, typeof p.next,
    O.prototype.toString.call(p), p[window.Symbol.iterator]() === p,
    GeneratorProto === O.getPrototypeOf(O.getPrototypeOf((function* () {})())) || "own proto",
    O.prototype.toString.call(proto), typeof proto.prototype, O.keys(proto).length);
log(errName(() => new proto()), errName(() => GeneratorProto.next.call({})));

// a generator can't resume itself while running
let self;
function* reenter() {
    yield self.next();
}
self = reenter();
log(errName(() => self.next()));

// yields inside expressions keep the surrounding operands
function* expr() {
    let r = [1, (yield "a"), 3 + (yield "b")];
    return r.join(",");
}
let e = expr();
e.next();
e.next(2);
log(show(e.next(4)));

// generator expressions and closures over loop variables
let gens = [];
for (let i = 0; i < 3; i++) gens.push((function* () { yield i; })());
log(gens.map((x) => x.next().value).join(","));

// nested yield in a loop with labelled continue
function* nested() {
    outer: for (let i = 0; i < 3; i++) {
        for (let j = 0; j < 3; j++) {
            if (j === 1) continue outer;
            yield i + "" + j;
        }
    }
}
log([...nested()].join(","));
