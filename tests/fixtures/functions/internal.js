// Functions only ever called directly run without a host wrapper; any other use keeps it.
let log = window.console.log;

// internal: direct calls, recursion, mutual recursion, closures
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
function isEven(n) { return n === 0 ? true : isOdd(n - 1); }
function isOdd(n) { return n === 0 ? false : isEven(n - 1); }
function counter() {
    let count = 0;
    function bump() { count++; return count; }
    bump(); bump();
    return bump();
}
log(fib(15), isEven(10), isOdd(7), counter());

// internal function called from an escaping one (callback from a native)
function double(x) { return x * 2; }
log([1, 2, 3].map(function (x) { return double(x) + 1; }).join(","));

// exceptions through internal functions
function boom(msg) { throw new window.Error(msg); }
function relay(msg) { try { boom(msg); } finally { log("finally", msg); } }
try { relay("a"); } catch (e) { log("caught", e.message); }

// `this` in a direct call is undefined (strict mode)
function whatIsThis() { return this; }
log(whatIsThis());

// escaping uses: each must still behave as a real function
function passed(x) { return x + 1; }
log([1, 2].map(passed).join(","));
function stored() { return "stored"; }
let obj = { stored: stored };
log(obj.stored(), typeof obj.stored, obj.stored.name, obj.stored.length);
function inArray() { return "array"; }
log([inArray][0]());
function typed() {}
log(typeof typed);
function Ctor(v) { this.v = v; }
log(new Ctor(3).v);
function returned() { return returned; }
log(returned() === returned);
function aliased() { return "alias"; }
let alias = (aliased);
log(alias());
function viaComma() { return "comma"; }
log((0, viaComma)());

// shadowing: the inner `shadow` escapes, the outer one is only called
function shadow() { return "outer"; }
function useShadow() {
    function shadow() { return "inner"; }
    return [shadow].map(function (f) { return f(); })[0];
}
log(shadow(), useShadow());

// IIFEs: plain, recursive by name, and leaking their own name
log((function () { return "iife"; })());
log((function f(n) { return n <= 1 ? 1 : n * f(n - 1); })(5));
log(typeof (function g() { return g; })());

// block-scoped declarations
{
    function inBlock() { return "block"; }
    log(inBlock());
}
switch (1) {
    case 1:
        function inCase() { return "case"; }
        log(inCase());
}
