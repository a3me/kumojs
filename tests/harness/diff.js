// Runs a fixture both in the kumo VM and directly in Node, and prints a JSON
// comparison of console output and thrown errors.
// Usage: node diff.js <source.js> <bytecode.kumo>
const fs = require('fs');
const nodeVm = require('vm');
const util = require('util');
const { VM } = require('../../src/vm/vm.js');

const [sourcePath, bytecodePath] = process.argv.slice(2);

const format = (args) => args.map((a) => (typeof a === 'string' ? a : util.inspect(a))).join(' ');
const errorName = (e) => (e && e.name) || String(e);

function runKumo() {
    const logs = [];
    const originalLog = console.log;
    console.log = (...args) => logs.push(format(args));
    let error = null;
    try {
        new VM([new Uint8Array(fs.readFileSync(bytecodePath))]).run();
    } catch (e) {
        error = { name: errorName(e), message: e && e.message };
    } finally {
        console.log = originalLog;
    }
    return { logs, error };
}

function runNode() {
    const logs = [];
    const sandbox = { console: { log: (...args) => logs.push(format(args)) } };
    let error = null;
    try {
        // kumo parses input as a module, which is always strict mode
        nodeVm.runInNewContext('"use strict";\n' + fs.readFileSync(sourcePath, 'utf8'), sandbox, {
            timeout: 5000,
        });
    } catch (e) {
        error = { name: errorName(e), message: e && e.message };
    }
    return { logs, error };
}

const kumo = runKumo();
const node = runNode();
const pass =
    JSON.stringify(kumo.logs) === JSON.stringify(node.logs) &&
    (kumo.error && kumo.error.name) === (node.error && node.error.name);

console.log(JSON.stringify({ pass, kumo, node }));
