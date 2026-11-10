let log = window.console.log;
function describe(x) {
    let out = [];
    switch (x) {
        case 1:
            out.push("one");
        case 2:
            out.push("two");
            break;
        case "3":
            out.push("string three");
            break;
        default:
            out.push("default");
        case 4:
            out.push("four");
    }
    return out.join("+");
}
log(describe(1), describe(2), describe(3), describe("3"), describe(4), describe(9));

function noDefault(x) {
    switch (x) {
        case 1:
            return "one";
    }
    return "none";
}
log(noDefault(1), noDefault(2));

// break inside a switch inside a loop only leaves the switch; continue goes to the loop
let out = [];
for (let i = 0; i < 4; i++) {
    switch (i) {
        case 1:
            continue;
        case 2:
            break;
        default:
            out.push(i);
    }
    out.push("after" + i);
}
log(out.join(","));

// case expressions are evaluated in order until one matches
let evaluated = [];
function c(v) { evaluated.push(v); return v; }
switch (2) {
    case c(1):
    case c(2):
    case c(3):
}
log(evaluated.join(","));
