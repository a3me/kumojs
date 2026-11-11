let log = window.console.log;
let json = window.JSON.stringify;

// arrays: holes, defaults, rest, nesting
let [a, , b = 5, ...rest] = [1, 2, undefined, 4, 5];
log(a, b, json(rest));
const [[x, y], [z = "dz"] = []] = [[1, 2]];
log(x, y, z);
var [v1, v2] = [10];
log(v1, v2);

// objects: renaming, defaults, computed keys, nesting, rest
let key = "dyn";
let { p, q: renamed, r = "dr", [key]: computed, "str key": s, 0: zero } =
    { p: 1, q: 2, dyn: 3, "str key": 4, 0: "z" };
log(p, renamed, r, computed, s, zero);
const { deep: { inner: [first, second = "d2"] }, ...others } =
    { deep: { inner: ["f"] }, o1: 1, o2: 2 };
log(first, second, json(others));
let { length } = "hello";
log(length);

// object rest skips used keys (also computed ones) and copies only own enumerable props
let proto = { inherited: 1 };
let src = window.Object.create(proto);
src.own = 1;
src.used = 2;
window.Object.defineProperty(src, "hidden", { value: 3, enumerable: false });
let { used, ...copy } = src;
log(used, json(copy));
let k2 = "a";
let { [k2]: aval, ...noA } = { a: 1, b: 2 };
log(aval, json(noA));

// iterables and lazy iteration (only as many values as needed, iterator closed)
let [c1, c2] = "hey";
let [s1, ...sRest] = new window.Set([1, 2, 3]);
log(c1, c2, s1, json(sRest));
let pulled = 0;
let closed = false;
let iterable = {};
iterable[window.Symbol.iterator] = () => ({
    next: () => ({ value: ++pulled, done: false }),
    return: () => { closed = true; return {}; },
});
let [it1, it2] = iterable;
log(it1, it2, pulled, closed);

// defaults are lazy and only used for undefined
let calls = 0;
let make = () => ++calls;
let [d1 = make(), d2 = make()] = [null];
let { e1 = make() } = { e1: 0 };
log(d1, d2, e1, calls);

// errors
let attempt = f => { try { f(); log("no error"); } catch (e) { log(e.name); } };
attempt(() => { let { a } = null; });
attempt(() => { let { a } = undefined; });
attempt(() => { let [a] = 5; });
attempt(() => { let [a] = {}; });
attempt(() => { const [c] = [1]; c = 2; });

// closures capture destructured bindings per iteration
let fns = [];
for (let [i, j] = [0, 10]; i < 2; i++) fns.push(() => i + j);
log(fns.map(f => f()).join(","));
