function makeCounter() {
    let count = 0;
    function inc() {
        count += 1;
        return count;
    }
    return inc;
}
let counter = makeCounter();
counter();
counter();
window.console.log(counter());

let other = makeCounter();
window.console.log(other(), counter());

function outer(a) {
    function middle(b) {
        function inner(c) {
            return a + b + c;
        }
        return inner(3);
    }
    return middle(2);
}
window.console.log(outer(1));
