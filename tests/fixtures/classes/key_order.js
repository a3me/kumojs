let log = window.console.log;
let order = [];
let key = (name) => {
    order.push("key " + name);
    return name;
};
let keyObj = (name) => ({
    toString() {
        order.push("toString " + name);
        return name;
    },
});

// every computed key (methods, fields, static or not) is evaluated in source order,
// before any static initialiser runs
class C {
    static [key("s1")] = order.push("init s1");
    [key("m1")]() {}
    [key("f1")] = 1;
    static [keyObj("s2")] = order.push("init s2");
    static [key("m2")]() {}
    static {
        order.push("static block");
    }
    static [key("s3")] = order.push("init s3");
}
log(order.join(", "));
log(typeof C.prototype.m1, typeof C.m2, C.s1, C.s2, C.s3, new C().f1);
