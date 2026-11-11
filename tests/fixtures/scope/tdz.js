let log = window.console.log;
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};

// reads and writes before the declaration throw, in the same function
log(errName(() => {
    let r = x;
    let x = 1;
}), errName(() => {
    x = 2;
    let x;
}), errName(() => {
    const c = c + 1;
}), errName(() => {
    typeof z;
    let z;
}));

// a hoisted function called too early throws; called after, it works
log(errName(() => {
    early();
    let v = 1;
    function early() {
        return v;
    }
}), (() => {
    let v = 1;
    function late() {
        return v;
    }
    return late();
})());

// closures made before the declaration check when they run
let make;
let result = (() => {
    let read = () => w;
    let write = (n) => (w = n);
    let before = errName(read) + "/" + errName(() => write(1));
    let w = 5;
    return before + " " + read() + " " + write(7) + " " + w;
})();
log(result);

// classes and const too
log(errName(() => {
    new K();
    class K {}
}), errName(() => {
    class E extends E {}
}), errName(() => {
    const f = () => k;
    f();
    const k = 1;
}));

// switch cases share one scope, so a case can skip the declaration
let sw = (n) => {
    switch (n) {
        case 0:
            let s = "zero";
            return s;
        case 1:
            return s;
    }
};
log(sw(0), errName(() => sw(1)));

// a for-of / for-in head can't use its own binding
log(errName(() => {
    for (let q of [q]) {}
}), errName(() => {
    for (const p in { p }) {}
}));

// each loop iteration starts a fresh dead zone
let loopResults = [];
for (let i = 0; i < 2; i++) {
    loopResults.push(errName(() => inner));
    let inner = i;
    loopResults.push(inner);
}
log(loopResults.join(","));

// after initialisation everything is normal, including recursion through the binding
const fact = (n) => (n <= 1 ? 1 : n * fact(n - 1));
let counter = 0;
function bump() {
    return ++counter;
}
log(fact(5), bump(), bump(), counter);

// var and parameters have no dead zone
log((() => {
    let a = typeof hoisted;
    var hoisted = 1;
    return a;
})());
