function t(v) {
    window.console.log("evaluated", v);
    return v;
}
let a = 0;
a ||= t(5);
window.console.log(a);
a ||= t("not reached");
window.console.log(a);
let b = 1;
b &&= t(7);
window.console.log(b);
let c = 0;
c &&= t("not reached");
window.console.log(c);
let d = null;
d ??= t(9);
window.console.log(d);
d ??= t("not reached");
window.console.log(d);
