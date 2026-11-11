let log = window.console.log;

// super.x as an update / compound / destructuring target writes to `this`
class Base {
    constructor() {
        this.n = 1;
    }
    get g() {
        return 10;
    }
}
Base.prototype.p = 5;
class Sub extends Base {
    run() {
        let out = [];
        out.push(super.p++, this.p, ++super.p, this.p, super.p--, this.p);
        super.q = 1;
        super.q += 4;
        super.q *= 2;
        super.r ??= "r";
        super.r ||= "no";
        super.r &&= super.r + "!";
        out.push(this.q, this.r, super.g + 1);
        [super.a, super.b] = [1, 2];
        ({ c: super.c } = { c: 3 });
        out.push(this.a, this.b, this.c, Base.prototype.a, Base.prototype.p);
        return out.join(" ");
    }
}
log(new Sub().run());

let proto = { x: 1 };
let obj = {
    __proto__: proto,
    bump() {
        super.x += 10;
        super.x++;
        return [this.x, proto.x].join(",");
    },
};
log(obj.bump());
