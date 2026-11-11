let log = window.console.log;
let Promise = window.Promise;
let O = window.Object;

// await of plain values, promises and thenables; return values resolve the promise
async function basics() {
    let a = await 1;
    let b = await Promise.resolve(2);
    let c = await { then(r) { r(3); } };
    return a + b + c;
}
let p = basics();
log(p instanceof Promise, O.prototype.toString.call(basics));
p.then((v) => log("basics", v));

// the body runs synchronously until the first await
let order = [];
async function sync() {
    order.push("start");
    await null;
    order.push("after await");
}
sync();
order.push("caller");
Promise.resolve().then(() => log(order.join(" ")));

// microtask interleaving matches native
async function tick(name, n) {
    for (let i = 0; i < n; i++) {
        log(name, i);
        await undefined;
    }
}
tick("a", 3);
tick("b", 3);
Promise.resolve().then(() => log("plain then 1")).then(() => log("plain then 2"));

// returning a promise takes extra ticks, like native
async function retPromise() {
    return Promise.resolve("ret");
}
async function retValue() {
    return "val";
}
retPromise().then(log);
retValue().then(log);

// throws become rejections, caught with try/catch or .catch
async function fails() {
    await null;
    throw new window.TypeError("async boom");
}
async function failsSync() {
    throw new window.RangeError("sync boom");
}
fails().catch((e) => log("caught", e.name, e.message));
let fsPromise = failsSync();
log("failsSync returned", fsPromise instanceof Promise);
fsPromise.catch((e) => log("caught", e.name));

async function handles() {
    try {
        await fails();
    } catch (e) {
        log("try/catch", e.message);
    } finally {
        log("finally");
    }
    try {
        await Promise.reject(new window.Error("rejected"));
    } catch (e) {
        return "recovered " + e.message;
    }
}
handles().then(log);

// a VM caller's try/catch doesn't see an async callee's throw
function caller() {
    try {
        let r = failsSync();
        return "no throw, got " + typeof r.then;
    } catch (e) {
        return "wrongly caught";
    }
}
log(caller());
failsSync().catch(() => {});

// async arrows keep `this` / `arguments`; async methods; class async methods with super
let obj = {
    v: 5,
    async m() {
        let f = async () => this.v + arguments.length;
        return await f();
    },
};
obj.m(1, 2).then((v) => log("method", v));
class Base {
    async get() {
        await null;
        return "base";
    }
}
class Sub extends Base {
    async get() {
        return (await super.get()) + "+sub";
    }
    static async s() {
        return "static";
    }
}
new Sub().get().then(log);
Sub.s().then(log);

// awaits in loops and expressions keep operands
async function loops() {
    let out = [];
    for (let i = 0; i < 3; i++) out.push(await i * 2);
    let sum = 1 + (await 2) + (await Promise.resolve(3));
    out.push(sum);
    let fns = [];
    for (let i = 0; i < 2; i++) {
        await null;
        fns.push(() => i);
    }
    out.push(fns.map((f) => f()).join("/"));
    return out.join(",");
}
loops().then(log);

// Promise.all with async functions
Promise.all([1, 2, 3].map(async (x) => (await x) * 10)).then((v) => log("all", v.join(",")));

// async functions can't be constructed
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};
log(errName(() => new basics()), typeof basics.prototype, O.getPrototypeOf(basics) === O.getPrototypeOf(async function () {}));

// an unhandled rejection
async function unhandledOne() {
    throw new window.SyntaxError("nobody catches this");
}
unhandledOne();
