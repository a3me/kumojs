let x = 1;
{
    let x = 2;
    window.console.log(x);
    {
        let x = 3;
        window.console.log(x);
    }
    window.console.log(x);
}
window.console.log(x);

// a closure keeps a block variable alive after the block ends
let getter;
{
    let hidden = "kept";
    function get() {
        return hidden;
    }
    getter = get;
}
{
    let reused = "other";
}
window.console.log(getter());

// shadowing window
function shadow() {
    let window = { console: { log: 1 } };
    return window.console.log;
}
window.console.log(shadow());
