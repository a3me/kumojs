function t(v) {
    console.log("evaluated", v);
    return v;
}
console.log(t(false) && t("not reached"));
console.log(t(1) && t(2));
console.log(t(1) || t("not reached"));
console.log(t(0) || t(3));
console.log(t(null) ?? t(4));
console.log(t(0) ?? t("not reached"));
console.log(t("") || t(0) || t("last"));
