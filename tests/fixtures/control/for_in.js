let log = window.console.log;
let obj = { a: 1, b: 2, c: 3 };
let keys = [];
for (let k in obj) keys.push(k + "=" + obj[k]);
log(keys.join(","));

function Base() {}
Base.prototype.inherited = true;
let child = new Base();
child.own = 1;
keys = [];
for (var key in child) keys.push(key);
log(keys.join(","));

let target = {};
for (target.last in { x: 1, y: 2 }) {}
log(target.last);

keys = [];
for (let k in [7, 8, 9]) {
    if (k === "1") continue;
    keys.push(k);
}
log(keys.join(","));

for (let k in null) log("never");
for (let k in undefined) log("never");
log("done");
