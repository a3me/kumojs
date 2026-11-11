// class getter and setter used from VM code, 1e5 each
class Box {
    constructor() {
        this._v = 0;
    }
    get v() {
        return this._v;
    }
    set v(x) {
        this._v = x;
    }
}
let b = new Box();
let total = 0;
for (let i = 0; i < 100000; i++) {
    b.v = i;
    total += b.v;
}
window.console.log(total);
