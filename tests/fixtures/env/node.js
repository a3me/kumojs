// env: node
globalThis.console.log(typeof window, typeof globalThis);
let isNode =
    typeof globalThis.process !== "undefined" &&
    globalThis.process.versions != null &&
    globalThis.process.versions.node != null;
globalThis.console.log("isNode", isNode);
