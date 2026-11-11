// loops, closures over per-iteration bindings, labelled break / continue
let sum = 0;
for (let i = 0; i < 3; i++) sum += i;
let fns = [];
for (let j = 0; j < 2; j++) fns.push(() => j);
outer: for (const k in { a: 1 }) {
    for (const v of [1, 2]) {
        if (v === 2) continue outer;
        if (k === "b") break outer;
    }
}
while (sum > 0) sum--;
