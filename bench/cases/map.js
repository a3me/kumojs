// a VM arrow called by a host native (Array.prototype.map), 1e5 calls
let arr = [];
for (let i = 0; i < 1000; i++) arr.push(i);
let total = 0;
for (let r = 0; r < 100; r++) {
    total += arr.map((x) => x * 2)[999];
}
window.console.log(total);
