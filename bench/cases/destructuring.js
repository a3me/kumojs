// array and object patterns, 1e5 each
let total = 0;
for (let i = 0; i < 100000; i++) {
    let [a, b] = [i, 1];
    let { x, y } = { x: a, y: b };
    total += x + y;
}
window.console.log(total);
