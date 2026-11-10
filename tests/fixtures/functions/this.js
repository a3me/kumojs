function getN() {
    return this.n;
}
let obj = { n: 5 };
obj.get = getN;
window.console.log(obj.get(), obj["get"](), (obj.get)());

let other = { n: 7, inner: { n: 8 } };
other.f = getN;
other.inner.f = getN;
window.console.log(other.f(), other.inner.f());

function plain() {
    return this;
}
window.console.log(plain());

function inc(by) {
    this.count = this.count + by;
    return this.count;
}
let counter = { count: 0 };
counter.inc = inc;
counter.inc(2);
window.console.log(counter.inc(3), counter.count);
