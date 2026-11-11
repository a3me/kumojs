// recursive VM-to-VM calls
function fib(n) {
    return n < 2 ? n : fib(n - 1) + fib(n - 2);
}
window.console.log(fib(25));
