let log = window.console.log;
let out = [];
let i = 0;
while (i < 5) {
    i++;
    if (i === 2) continue;
    if (i === 4) break;
    out.push(i);
}
log(out.join(","));

out = [];
let j = 10;
do {
    out.push(j);
    j++;
} while (j < 3);
log(out.join(","));

out = [];
for (let n = 0; n < 10; n += 1) {
    if (n % 2) continue;
    if (n > 6) break;
    out.push(n);
}
log(out.join(","));

let count = 0;
for (;;) {
    if (++count >= 3) break;
}
log(count);

// var in for is function-scoped
function lastIndex() {
    for (var q = 0; q < 3; q++) {}
    return q;
}
log(lastIndex());

// nested loops with labels
out = [];
outer: for (let a = 0; a < 3; a++) {
    inner: for (let b = 0; b < 3; b++) {
        if (b === 1) continue outer;
        if (a === 2) break outer;
        out.push(a + ":" + b);
    }
}
log(out.join(" "));

block: {
    log("in block");
    if (true) break block;
    log("unreachable");
}
log("after block");

// closures over a var loop variable all see the final value
let fns = [];
for (var v = 0; v < 3; v++) {
    fns.push(function () { return v; });
}
log(fns[0](), fns[2]());
