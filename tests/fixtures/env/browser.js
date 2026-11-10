window.console.log(typeof window, typeof globalThis, window === globalThis);
let isNode =
    typeof globalThis.process !== "undefined" &&
    globalThis.process.versions != null &&
    globalThis.process.versions.node != null;
window.console.log("isNode", isNode);
