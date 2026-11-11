// Runs a fixture both in the kumo VM and directly in Node, and prints a JSON
// comparison of console output and thrown errors.
// Usage: node diff.js <source.js> <bytecode.kumo>
//
// Both sides run in identical fresh sandboxes, so they see the same globals:
// - default: browser-like, `window` is the sandbox global
// - first line `// env: node`: no `window`, and Node's `process` is exposed
// Each side's promise jobs finish before comparing; unhandled rejections are compared too.
const fs = require('fs');
const path = require('path');
const nodeVm = require('vm');
const util = require('util');

const [sourcePath, bytecodePath] = process.argv.slice(2);
const source = fs.readFileSync(sourcePath, 'utf8');
const nodeEnv = source.trimStart().startsWith('// env: node');
const vmSource = fs.readFileSync(path.join(__dirname, '../../src/vm/vm.js'), 'utf8');

const format = (args) => args.map((a) => (typeof a === 'string' ? a : util.inspect(a))).join(' ');
const TIMEOUT_MS = 5000;

function makeContext(logs) {
    const sandbox = { console: { log: (...args) => logs.push(format(args)) } };
    if (nodeEnv) {
        sandbox.process = process;
    }
    const context = nodeVm.createContext(sandbox);
    if (!nodeEnv) {
        nodeVm.runInContext('globalThis.window = globalThis;', context);
    }
    return context;
}

// Unhandled promise rejections, by error name, for the side that's running.
let unhandled = [];
process.on('unhandledRejection', (e) => unhandled.push((e && e.name) || String(e)));

// Let promise jobs (and the rejection tracking after them) finish.
const settle = () => new Promise((resolve) => setImmediate(() => setImmediate(resolve)));

// Run one side and wait for its async work, so logs after an `await` count too.
async function run(fn) {
    const logs = [];
    let error = null;
    unhandled = [];
    try {
        fn(makeContext(logs));
    } catch (e) {
        error = { name: (e && e.name) || String(e), message: e && e.message };
    }
    await settle();
    return { logs, error, unhandled };
}

(async () => {
const kumo = await run((context) => {
    context.__kumoBytecode = Array.from(fs.readFileSync(bytecodePath));
    nodeVm.runInContext(
        vmSource +
            '\n;{ const code = new Uint8Array(globalThis.__kumoBytecode);' +
            ' delete globalThis.__kumoBytecode; new VM([code]).run(); }',
        context,
        { timeout: TIMEOUT_MS }
    );
});

// kumo parses input as a module, which is always strict mode
const node = await run((context) =>
    nodeVm.runInContext('"use strict";\n' + source, context, { timeout: TIMEOUT_MS })
);

const pass =
    JSON.stringify(kumo.logs) === JSON.stringify(node.logs) &&
    (kumo.error && kumo.error.name) === (node.error && node.error.name) &&
    JSON.stringify(kumo.unhandled) === JSON.stringify(node.unhandled);

console.log(JSON.stringify({ pass, kumo, node }));
})();
