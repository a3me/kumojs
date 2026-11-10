let log = window.console.log;
let money = { valueOf: function () { return 42; }, toString: function () { return "$42"; } };
log(money + 1, money * 2, "" + money, money > 41);
log(window.String(money), window.JSON.stringify({ a: 1, b: [1, 2] }, function (k, v) {
    return typeof v === "number" ? v * 10 : v;
}));
let bound = function (a, b) { return this.base + a + b; }.bind({ base: 100 }, 1);
log(bound(2));
function variadic() { return window.Array.prototype.slice.call(arguments, 1).join("-"); }
log(variadic("skip", "a", "b", "c"));
function outer() {
    function inner() { return arguments.length; }
    return [arguments.length, inner(1, 2, 3)];
}
log(outer("x"));
log(typeof function () {}, delete {}.x, (function () {}).length, (function (a, b, c) {}).length);
let long = "";
for (let i = 0; i < 5000; i++) long += "é";
let copy = "éééééééééééééééééééééééééééééééééééééééééééééééééééé";
log(long.length, copy.length);
log([1, 2, 3].reduce(function (acc, x) { return acc + x; }, 0));
let ctor = function Thing(v) { this.v = v; };
log([1, 2].map(function (v) { return new ctor(v).v; }).join(","));
