// a derived class calling super(...) and a super method, 1e5 instances
class Shape {
    constructor(size) {
        this.size = size;
    }
    area() {
        return this.size * this.size;
    }
}
class Square extends Shape {
    tag = "sq";
    constructor(size) {
        super(size);
    }
    area() {
        return super.area() + 1;
    }
}
let total = 0;
for (let i = 0; i < 100000; i++) {
    total += new Square(i % 10).area();
}
window.console.log(total);
