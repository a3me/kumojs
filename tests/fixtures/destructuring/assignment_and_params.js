let log = window.console.log;
let json = window.JSON.stringify;

// assignment patterns, including member targets and swapping
let a = 1, b = 2;
[a, b] = [b, a];
log(a, b);
let o = {};
let arr = [];
({ x: o.x, y: arr[0], z: o["z"] = "dz" } = { x: 1, y: 2 });
log(json(o), json(arr));
let p, q, rest;
({ p, q = 3, ...rest } = { p: 1, r: 4, s: 5 });
log(p, q, json(rest));
[o.first, ...o.others] = [1, 2, 3];
log(json(o));

// the assignment's value is the right-hand side
let rhs = [1, 2];
let result = ([a, b] = rhs);
log(result === rhs);
let n;
log(json({ n } = { n: 7 }), n);

// parameters
function point({ x, y = 0 }, [first, ...more] = [], ...extra) {
    return [x, y, first, json(more), json(extra)].join(" ");
}
log(point({ x: 1 }), point({ x: 1, y: 2 }, [3, 4, 5], "e1", "e2"), point.length);
const area = ({ w, h }) => w * h;
log(area({ w: 3, h: 4 }));
function withDefault({ name } = { name: "anon" }) { return name; }
log(withDefault(), withDefault({ name: "kumo" }));
function nested({ a: { b: [c] } }) { return c; }
log(nested({ a: { b: ["deep"] } }));
function restPattern(...[first, second]) { return first + second; }
log(restPattern(1, 2, 3));
function captured({ v }) { return () => v; }
log(captured({ v: "cap" })());
log([[1, 2], [3, 4]].map(([l, r]) => l * r).join(","));
log(window.Object.entries({ a: 1, b: 2 }).map(([k, v]) => k + v).join(","));

// catch parameter patterns
try { throw { code: 42, info: ["i"] }; } catch ({ code, info: [i] }) { log(code, i); }

// for-in with a pattern head
for (const [ch] in { abc: 1 }) log(ch);
