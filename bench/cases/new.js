// `new` on a plain VM constructor function, 1e5 times
function Point(x, y) {
    this.x = x;
    this.y = y;
}
Point.prototype.sum = function () {
    return this.x + this.y;
};
let total = 0;
for (let i = 0; i < 100000; i++) {
    total += new Point(i, 1).sum();
}
window.console.log(total);
