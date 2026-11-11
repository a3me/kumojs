let log = window.console.log;
let call = function (fns) { return fns.map(function (f) { return f(); }).join(","); };

// each iteration of `for (let ...)` has its own binding
let fns = [];
for (let i = 0; i < 3; i++) fns.push(function () { return i; });
log(call(fns));

// the update runs on the new binding; the body's changes carry over
fns = [];
for (let i = 0; i < 6; i++) { fns.push(function () { return i; }); i++; }
log(call(fns));

// closures can also write to their iteration's binding
let incs = [];
let gets = [];
for (let i = 0; i < 2; i++) {
    incs.push(function () { i += 10; });
    gets.push(function () { return i; });
}
incs[0]();
log(call(gets));

// continue still gets a fresh binding
fns = [];
for (let i = 0; i < 4; i++) {
    if (i % 2) continue;
    fns.push(function () { return i; });
}
log(call(fns));

// closures in the initializer keep the first binding
let first;
for (let i = 0, f = function () { return i; }; i < 3; i++) first = f;
log(first());

// several loop variables, nested loops
fns = [];
for (let i = 0, j = 10; i < 2; i++, j--) {
    for (let k = 0; k < 2; k++) fns.push(function () { return i + ":" + j + ":" + k; });
}
log(call(fns));

// `let` inside a loop body is fresh each time the block runs
fns = [];
let n = 0;
while (n < 3) { let v = n * 2; fns.push(function () { return v; }); n++; }
log(call(fns));
fns = [];
n = 0;
do { const c = "c" + n; fns.push(function () { return c; }); n++; } while (n < 2);
log(call(fns));

// for-in with let / const
fns = [];
for (let key in { a: 1, b: 2 }) fns.push(function () { return key; });
for (const key in { c: 1 }) fns.push(function () { return key; });
log(call(fns));

// `var` is one binding for the whole function
fns = [];
for (var w = 0; w < 3; w++) fns.push(function () { return w; });
log(call(fns));

// catch parameters and parameters captured by closures
function makeCounter(start) {
    return function () { start++; return start; };
}
let c1 = makeCounter(5);
c1();
log(c1(), makeCounter(0)());
let caught;
try { throw "boom"; } catch (e) { caught = function () { return e; }; }
log(caught());

// a deeper closure capturing through a middle function
function outer() {
    let x = 1;
    function middle() {
        return function () { x++; return x; };
    }
    return [middle(), function () { return x; }];
}
let pair = outer();
pair[0]();
pair[0]();
log(pair[1]());

// block-level function declarations inside a loop
fns = [];
for (let i = 0; i < 2; i++) {
    function get() { return i; }
    fns.push(get);
}
log(call(fns));
