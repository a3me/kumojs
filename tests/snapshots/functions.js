// direct-call (internal) vs escaping functions, defaults, rest, arguments, closures
function add(a, b = 1) {
    return a + b;
}
function escapes(...rest) {
    return arguments.length + rest.length;
}
let counter = (() => {
    let n = 0;
    return () => ++n;
})();
add(1);
window.setTimeout = escapes;
counter();
