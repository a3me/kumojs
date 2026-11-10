let log = window.console.log;
function f1() {
    try {
        return "try";
    } finally {
        log("finally runs on return");
    }
}
log(f1());

function f2() {
    try {
        return "try";
    } finally {
        return "finally wins";
    }
}
log(f2());

function f3() {
    try {
        throw "oops";
    } catch (e) {
        return "catch " + e;
    } finally {
        log("finally after catch");
    }
}
log(f3());

function f4() {
    try {
        try {
            throw "inner";
        } finally {
            log("inner finally");
        }
    } catch (e) {
        return "outer caught " + e;
    }
}
log(f4());

let out = [];
for (let i = 0; i < 3; i++) {
    try {
        if (i === 1) continue;
        if (i === 2) break;
        out.push("body" + i);
    } finally {
        out.push("finally" + i);
    }
}
log(out.join(","));

// finally sees its own scope, not the try block's
function scopes() {
    let x = "outer";
    try {
        let x = "inner";
        return x;
    } finally {
        log("finally x =", x);
    }
}
log(scopes());

function catchThrows() {
    try {
        throw 1;
    } catch (e) {
        throw "from catch";
    } finally {
        log("finally after catch throws");
    }
}
try {
    catchThrows();
} catch (e) {
    log(e);
}

function nested() {
    try {
        try {
            return "value";
        } finally {
            log("first");
        }
    } finally {
        log("second");
    }
}
log(nested());
