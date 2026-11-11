let log = window.console.log;
let Symbol = window.Symbol;

// arrays, strings (by code point), Maps, Sets, arguments
let out = [];
for (let x of [1, 2, 3]) out.push(x);
for (const ch of "a😀b") out.push(ch);
for (const [k, v] of new window.Map([["k", 1]])) out.push(k + v);
for (let s of new window.Set([5, 5, 6])) out.push(s);
(function () {
    for (let a of arguments) out.push(a);
})(7, 8);
log(out.join(","));

// holes read as undefined, and the array is re-read each step
let arr = [1, , 3];
let seen = [];
for (let v of arr) {
    seen.push(v);
    if (seen.length === 1) arr.push(4);
}
log(seen.join("|"), seen.length);

// a hand-written iterator: records next / return calls
function counter(limit, events) {
    return {
        [Symbol.iterator]() {
            let i = 0;
            return {
                next() {
                    events.push("next");
                    return { value: i, done: i++ >= limit };
                },
                return() {
                    events.push("return");
                    return {};
                },
            };
        },
    };
}

// running to the end doesn't call return()
let ev = [];
for (let v of counter(2, ev)) ev.push(v);
log(ev.join(" "));

// break, labelled break / continue, return and throw all close it; continue doesn't
ev = [];
for (let v of counter(5, ev)) {
    if (v === 0) continue;
    if (v === 2) break;
    ev.push(v);
}
log(ev.join(" "));

ev = [];
outer: for (let a of counter(3, ev)) {
    for (let b of counter(3, ev)) {
        if (b === 1) continue outer;
        if (a === 1) break outer;
    }
}
log(ev.join(" "));

ev = [];
let found = (() => {
    for (let v of counter(5, ev)) if (v === 1) return "found " + v;
})();
log(found, ev.join(" "));

ev = [];
try {
    for (let v of counter(5, ev)) if (v === 1) throw new window.Error("boom");
} catch (e) {
    ev.push(e.message);
}
log(ev.join(" "));

// a throwing return() is ignored when leaving by a throw, but not by a break
let badReturn = {
    [Symbol.iterator]() {
        return {
            next: () => ({ value: 1, done: false }),
            return() {
                throw new window.Error("from return");
            },
        };
    },
};
try {
    for (let v of badReturn) throw new window.Error("body");
} catch (e) {
    log(e.message);
}
try {
    for (let v of badReturn) break;
} catch (e) {
    log(e.message);
}

// finally blocks and iterator closing run in order
ev = [];
for (let v of counter(3, ev)) {
    try {
        if (v === 1) break;
    } finally {
        ev.push("finally " + v);
    }
}
log(ev.join(" "));

// a throwing next() doesn't call return()
let badNext = {
    [Symbol.iterator]() {
        return {
            next() {
                throw new window.Error("next failed");
            },
            return() {
                log("should not run");
                return {};
            },
        };
    },
};
try {
    for (let v of badNext) {}
} catch (e) {
    log(e.message);
}

// destructuring and member targets
let pairs = [[1, { y: 2 }], [3, { y: 4 }]];
let sums = [];
for (const [x, { y }] of pairs) sums.push(x + y);
let o = {};
for (o.last of [1, 2, 3]);
let i;
for (i of "xy");
for ([o.a, o.b] of [[5, 6]]);
log(sums.join(","), o.last, i, o.a, o.b);

// each iteration's let binding is captured separately
let fns = [];
for (let v of [1, 2, 3]) fns.push(() => v);
log(fns.map((f) => f()).join(","));

// not iterable / bad iterators
let errName = (fn) => {
    try {
        fn();
        return "no error";
    } catch (e) {
        return e.name;
    }
};
log(errName(() => { for (let v of 5); }), errName(() => { for (let v of {}); }),
    errName(() => { for (let v of null); }),
    errName(() => { for (let v of { [Symbol.iterator]: () => 1 }); }),
    errName(() => { for (let v of { [Symbol.iterator]: () => ({ next: () => 1 }) }); }));
