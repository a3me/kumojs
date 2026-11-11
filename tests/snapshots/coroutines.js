// generators, yield*, async functions, for await, optional chaining, TDZ checks
function* gen() {
    try {
        yield 1;
        yield* [2, 3];
    } finally {
        early();
    }
}
async function load(p) {
    let v = await p;
    for await (const x of gen()) v += x;
    return v?.toString?.();
}
function early() {
    return late;
}
let late = load(1);
