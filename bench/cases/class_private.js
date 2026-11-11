// class_base with private fields, a private method and a private getter, 1e5 instances
class Counter {
    #count = 0;
    #step = 1;
    constructor(start) {
        this.#count = start;
    }
    get #next() {
        return this.#count + this.#step;
    }
    #advance() {
        this.#count = this.#next;
    }
    inc() {
        this.#advance();
        return this.#count;
    }
}
let total = 0;
for (let i = 0; i < 100000; i++) {
    total += new Counter(i).inc();
}
window.console.log(total);
