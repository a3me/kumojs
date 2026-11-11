let log = window.console.log;

// default values
function greet(name, greeting = "hello") { return greeting + " " + name; }
log(greet("a"), greet("b", "hi"), greet("c", undefined), greet("d", null));
log(greet.length);

// defaults can use earlier parameters and outer variables, and run each call
let calls = 0;
let next = () => ++calls;
function seq(a, b = a * 2, c = next()) { return [a, b, c].join(","); }
log(seq(1), seq(1), seq(1, 5, 9), calls);

// closures in defaults see the parameter
function lazy(x, get = () => x) { x = "changed"; return get(); }
log(lazy("orig"));

// rest parameters
function rest(first, ...others) { return first + ":" + others.join("|") + ":" + others.length; }
log(rest(1), rest(1, 2), rest(1, 2, 3), rest.length);
let collect = (...all) => all;
log(collect().length, collect(1, 2).join(","), window.Array.isArray(collect()));
function restWithArguments(a, ...more) { return arguments.length + "/" + more.length; }
log(restWithArguments(1, 2, 3));

// captured rest and defaults
function capturedRest(...xs) { return () => xs.length; }
log(capturedRest(1, 2, 3)());
function capturedDefault(v = 5) { return () => v; }
log(capturedDefault()(), capturedDefault(7)());

// length stops at the first default or rest
log(((a, b = 1, c) => 0).length, ((a, ...b) => 0).length, (function (a, b) {}).length);
