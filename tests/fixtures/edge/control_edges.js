let log = window.console.log;
// break inside finally inside a loop
let out = [];
for (let i = 0; i < 5; i++) {
    try {
        out.push(i);
    } finally {
        if (i === 2) break;
    }
}
log(out.join(","));

// return from for-in inside try/finally
function findKey(obj, value) {
    try {
        for (let k in obj) {
            if (obj[k] === value) return k;
        }
        return null;
    } finally {
        log("search done");
    }
}
log(findKey({ a: 1, b: 2 }, 2), findKey({}, 1));

// labeled continue in do-while
let n = 0;
let hits = [];
outer: do {
    n++;
    for (let j = 0; j < 3; j++) {
        if (j === 1) continue outer;
        hits.push(n + "." + j);
    }
} while (n < 3);
log(hits.join(" "));

// let and functions inside switch cases
function sw(x) {
    switch (x) {
        case 1: {
            let y = "block one";
            return y;
        }
        case 2:
            return helper();
            function helper() { return "hoisted in case"; }
    }
}
log(sw(1), sw(2));

// deep recursion
function fib(k) { return k < 2 ? k : fib(k - 1) + fib(k - 2); }
log(fib(20));

// handlers in different frames
function level3() { throw "L3"; }
function level2() {
    try {
        level3();
    } finally {
        log("level2 finally");
    }
}
function level1() {
    try {
        level2();
    } catch (e) {
        return "level1 caught " + e;
    }
}
log(level1());

// exception thrown and caught inside a VM function called from a native
log([1, 2, 3].map(function (x) {
    try {
        if (x === 2) throw "two";
        return x;
    } catch (e) {
        return e;
    }
}).join(","));

// a native comparator that throws out of sort
try {
    [3, 1, 2].sort(function () { throw "sort failed"; });
} catch (e) {
    log(e);
}

// VM still works normally after exceptions crossed native frames
let total = 0;
for (let i = 0; i < 3; i++) total += [i].map(function (v) { return v * 10; })[0];
log(total);
