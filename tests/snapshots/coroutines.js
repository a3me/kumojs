// async functions, optional chaining, TDZ checks
async function load(p) {
    let v = await p;
    for (const x of [1, 2]) v += x;
    return v?.toString?.();
}
function early() {
    return late;
}
let late = load(1);
