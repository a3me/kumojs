// a VM generator driven by for-of (1e5 values) and by calling next() (1e5 values)
function* range(n) {
    for (let i = 0; i < n; i++) yield i;
}
let total = 0;
for (let v of range(100000)) total += v;
let g = range(100000);
for (let r = g.next(); !r.done; r = g.next()) total += r.value;
window.console.log(total);
