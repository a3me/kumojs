function t(v) {
    window.console.log("evaluated", v);
    return v;
}
window.console.log(t(false) && t("not reached"));
window.console.log(t(1) && t(2));
window.console.log(t(1) || t("not reached"));
window.console.log(t(0) || t(3));
window.console.log(t(null) ?? t(4));
window.console.log(t(0) ?? t("not reached"));
window.console.log(t("") || t(0) || t("last"));
