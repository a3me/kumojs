// skip: per-iteration `let` bindings in loops need real environments (Phase 2)
let fns = [];
for (let i = 0; i < 3; i++) fns.push(function () { return i; });
window.console.log(fns[0](), fns[1](), fns[2]());
