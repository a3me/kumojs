let log = window.console.log;
let attempt = function (f) {
    try { f(); log("no error"); } catch (e) { log(e.name, e.message); }
};

const c = 1;
attempt(function () { c = 2; });
attempt(function () { c += 1; });
attempt(function () { c++; });
attempt(function () { --c; });
attempt(function () { [c] = [5]; });
log(c);

// the right-hand side still runs before the throw
let ran = false;
attempt(function () { c = (ran = true); });
log(ran);

// an assignment that never runs is fine
if (false) c = 3;
log(c);

// const in loops: initialised fresh each iteration, but can't be updated
for (const k in { a: 1, b: 2 }) log(k);
attempt(function () { for (const i = 0; i < 2; i++) log(i); });

// a named function expression can't reassign its own name
attempt(function () { (function f() { f = 1; })(); });

// const declared locally or captured by a closure
function local() { const x = 1; x = 2; }
attempt(local);
function captured() { const y = 1; return function () { y = 2; }; }
attempt(captured());
