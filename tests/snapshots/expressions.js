// literals, destructuring, spread, templates, compound / logical assignment
let o = { a: 1, b: [2, 3], ...{ c: 4 }, get d() { return 5; } };
let { a, b: [x, ...ys], e = 6 } = o;
let s = `${a}-${x}`;
o.a ??= 7;
o.a += ys.length ** 2;
let big = 10n;
let t = typeof o === "object" && o instanceof window.Object;
