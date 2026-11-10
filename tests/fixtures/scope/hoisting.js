// functions can be called before their declaration
window.console.log(double(21));
function double(n) {
    return twice(n);
}
function twice(n) {
    return n + n;
}

// closures see variables declared later in the enclosing scope
function readLater() {
    return later;
}
let later = "declared after";
window.console.log(readLater());

// `var` is hoisted to the function body from nested blocks
function varInBlock() {
    {
        var inner = 5;
    }
    return inner;
}
window.console.log(varInBlock());

// `var x;` does not reset an existing value
function redeclare(a) {
    var a;
    return a;
}
window.console.log(redeclare(3));
