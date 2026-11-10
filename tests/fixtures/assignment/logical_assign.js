function t(v) {
    console.log("evaluated", v);
    return v;
}
let a = 0;
a ||= t(5);
console.log(a);
a ||= t("not reached");
console.log(a);
let b = 1;
b &&= t(7);
console.log(b);
let c = 0;
c &&= t("not reached");
console.log(c);
let d = null;
d ??= t(9);
console.log(d);
d ??= t("not reached");
console.log(d);
