let log = window.console.log;
let add = function (a, b) { return a + b; };
log(add(2, 3), typeof add, add.length, add.name === "" || add.name === "add");

let fact = function f(n) { return n <= 1 ? 1 : n * f(n - 1); };
log(fact(5), fact.name);

log((function () { return "iife"; })());
log([1, 2, 3].map(function (x) { return x * 2; }).join(","));
log([3, 1, 2].sort(function (a, b) { return a - b; }).join(","));

function sum() {
    let total = 0;
    for (let i = 0; i < arguments.length; i++) total += arguments[i];
    return total;
}
log(sum(1, 2, 3, 4), sum());

function extra(a) {
    var b;
    return [a, b, arguments.length];
}
log(extra(1, 2, 3));

function greet(greeting) { return greeting + ", " + this.name; }
let person = { name: "kumo" };
log(greet.call(person, "hi"), greet.apply(person, ["hey"]), greet.bind(person)("yo"));

function plain() { return this; }
log(plain());

let counter = { count: 0, inc: function () { this.count++; return this; } };
counter.inc().inc();
log(counter.count);
