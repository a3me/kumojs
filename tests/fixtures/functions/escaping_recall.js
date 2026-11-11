// A VM function stored on `window` becomes host-callable; calling it back (here, through
// `window`) round-trips into the VM and must behave like a plain function does in Node.
let log = window.console.log;

function mul(a, b) {
    return a * b;
}
window.c = mul;
log(window.c(2, 5), window.c.name, window.c.length, typeof window.c);

// a closure that escapes to the host keeps its captured state across host calls
function makeCounter() {
    let count = 0;
    return function () {
        count += 1;
        return count;
    };
}
window.counter = makeCounter();
log(window.counter(), window.counter(), window.counter());

// the escaped function used with host reflection (call / apply / bind / new / instanceof)
function Point(x) {
    this.x = x;
}
window.Point = Point;
let p = new window.Point(7);
log(p.x, p instanceof window.Point);
log(window.c.call(null, 3, 4), window.c.apply(null, [3, 4]), window.c.bind(null, 6)(7));
