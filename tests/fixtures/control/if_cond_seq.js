let log = window.console.log;
function classify(n) {
    if (n < 0) {
        return "negative";
    } else if (n === 0) {
        return "zero";
    } else if (n < 10) return "small";
    else return "large";
}
log(classify(-5), classify(0), classify(3), classify(50));
log(1 > 2 ? "a" : "b", 0 ? "x" : "" ? "y" : "z");
let x = (1, 2, 3);
log(x);
let calls = 0;
function bump() { calls += 1; return calls; }
log(true ? bump() : bump(), calls);
if (0) log("never"); else log("else branch");
