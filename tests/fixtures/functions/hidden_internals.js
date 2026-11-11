// VM functions handed to the host look like plain functions: no extra own properties
let log = window.console.log;
let R = window.Reflect;
let O = window.Object;

function f(a) { return a; }
let g = f;
let arrow = (x) => x;
let obj = { m() {}, get acc() { return 1; } };
class C { method() {} static s() {} }
class D extends C {}

let fns = [g, arrow, obj.m, O.getOwnPropertyDescriptor(obj, "acc").get, C, D, C.prototype.method, C.s];
for (let i = 0; i < fns.length; i++) {
    let fn = fns[i];
    let keys = [];
    for (let k in fn) keys.push(k);
    log(i, R.ownKeys(fn).join(","), O.getOwnPropertySymbols(fn).length, keys.length, O.keys(fn).length);
}
log(g.virtualFunction, "virtualFunction" in g);
