// env: node
// a bare `window` reference throws ReferenceError when the host has none
globalThis.console.log("before");
window.console.log("unreachable");
