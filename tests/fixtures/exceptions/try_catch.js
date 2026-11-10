let log = window.console.log;
try {
    throw "plain";
} catch (e) {
    log("caught", e);
}

try {
    null.x;
} catch (e) {
    log(e instanceof window.TypeError);
}

function thrower() { throw new window.Error("deep"); }
function middle() { thrower(); log("unreachable"); }
try {
    middle();
} catch (e) {
    log("caught", e.message);
}

// exceptions thrown inside a native callback reach a VM handler
try {
    [1, 2, 3].forEach(function (x) {
        if (x === 2) throw "from callback " + x;
    });
} catch (e) {
    log(e);
}

// nested, rethrow
try {
    try {
        throw 1;
    } catch (e) {
        log("inner", e);
        throw e + 1;
    }
} catch (e) {
    log("outer", e);
}

try {
    throw 0;
} catch {
    log("no binding");
}

// handler state stays correct after catching
let total = 0;
for (let i = 0; i < 5; i++) {
    try {
        if (i % 2) throw i;
        total += i;
    } catch (e) {
        total += 100;
    }
}
log(total);

function uncaught() { throw new window.RangeError("bad"); }
uncaught();
