let log = window.console.log;

// spread in calls
let nums = [3, 1, 4, 1, 5];
log(window.Math.max(...nums), window.Math.min(0, ...nums, -1));
function three(a, b, c) { return a + "-" + b + "-" + c; }
log(three(...["x", "y", "z"]), three("a", ...["b"], "c"), three(...[]));

// spread in array literals, holes, strings and other iterables
let a = [1, 2];
log([...a, 3, ...a].join(","), [...a].length, [..."hey"].join("|"));
log([0, , ...a].length, 1 in [0, , ...a]);
log([...new window.Set([1, 1, 2])].join(","), [...new window.Map([["k", "v"]])][0].join("="));

// spread into method calls keeps `this`
let o = { v: 10, add: function (x, y) { return this.v + x + y; } };
log(o.add(...[1, 2]));
let arr = [];
arr.push(...[1, 2, 3]);
log(arr.length);

// spread with `new`
function P(x, y) { this.sum = x + y; }
log(new P(...[2, 3]).sum);
log(new window.Date(...[2020, 0, 2]).getDate());

// spreading `arguments`
function pass() { return three(...arguments); }
log(pass(7, 8, 9));

// spreading a non-iterable throws a TypeError
try { [...5]; } catch (e) { log(e instanceof window.TypeError); }
