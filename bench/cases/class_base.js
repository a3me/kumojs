// a base class with fields and a method, 1e5 instances
class Counter {
    count = 0;
    step = 1;
    constructor(start) {
        this.count = start;
    }
    inc() {
        this.count += this.step;
        return this.count;
    }
}
let total = 0;
for (let i = 0; i < 100000; i++) {
    total += new Counter(i).inc();
}
window.console.log(total);
