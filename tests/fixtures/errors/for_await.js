// compile-error: for await
async function f() {
    for await (const x of []) {}
}
