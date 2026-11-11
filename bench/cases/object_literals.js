// plain literals (1e5) and literals with accessors (1e5)
let total = 0;
for (let i = 0; i < 100000; i++) {
    let o = { a: i, b: 1, c: "x" };
    total += o.a + o.b;
}
for (let i = 0; i < 100000; i++) {
    let o = { v: i, get double() { return this.v * 2; } };
    total += o.v;
}
window.console.log(total);
