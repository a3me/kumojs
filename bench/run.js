// Benchmarks: compiles each bench/cases/*.js with kumoc, then times it in the kumo VM and
// natively in Node. Usage: node bench/run.js [case-name-filter]
// Each run is cold (a fresh Node process, like a script running once on a page): repeated
// runs in one process get slower as V8 re-optimises the VM loop for each new VM.
'use strict';
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const RUNS = 5;
const root = path.join(__dirname, '..');

// child: `node run.js --child <kumo|node> <source.js> [bytecode]`, prints { ms, out }
if (process.argv[2] === '--child') {
    const [mode, sourcePath, bytecodePath] = process.argv.slice(3);
    // scripts are written against `window`, as in a browser
    globalThis.window = globalThis;
    let run;
    if (mode === 'kumo') {
        const { VM } = require(path.join(root, 'src/vm/vm.js'));
        const bytecode = new Uint8Array(fs.readFileSync(bytecodePath));
        run = () => new VM([bytecode]).run();
    } else {
        run = new Function('"use strict";\n' + fs.readFileSync(sourcePath, 'utf8'));
    }
    const logs = [];
    const realLog = console.log;
    console.log = (...args) => logs.push(args.join(' '));
    const start = process.hrtime.bigint();
    run();
    const ms = Number(process.hrtime.bigint() - start) / 1e6;
    console.log = realLog;
    console.log(JSON.stringify({ ms, out: logs.join('\n') }));
    process.exit(0);
}

execFileSync('cargo', ['build', '--release', '--quiet', '--bin', 'kumoc'], { cwd: root, stdio: 'inherit' });
const kumoc = path.join(root, 'target/release/kumoc');
const tmp = fs.mkdtempSync(path.join(require('os').tmpdir(), 'kumo-bench-'));

function median(args) {
    const runs = [];
    let out;
    for (let i = 0; i < RUNS; i++) {
        const r = JSON.parse(execFileSync(process.execPath, [__filename, '--child', ...args], { encoding: 'utf8' }));
        runs.push(r.ms);
        out = r.out;
    }
    runs.sort((a, b) => a - b);
    return { ms: runs[RUNS >> 1], out };
}

const filter = process.argv[2] || '';
const casesDir = path.join(__dirname, 'cases');
const cases = fs.readdirSync(casesDir).filter((f) => f.endsWith('.js') && f.includes(filter)).sort();

console.log(`${'case'.padEnd(18)}${'kumo ms'.padStart(10)}${'node ms'.padStart(10)}${'ratio'.padStart(9)}`);
for (const file of cases) {
    const sourcePath = path.join(casesDir, file);
    const bytecodePath = path.join(tmp, file + '.kumo');
    execFileSync(kumoc, [sourcePath, bytecodePath]);
    const kumo = median(['kumo', sourcePath, bytecodePath]);
    const native = median(['node', sourcePath]);
    const name = file.replace(/\.js$/, '');
    const mismatch = kumo.out !== native.out ? `  OUTPUT DIFFERS: kumo=${kumo.out} node=${native.out}` : '';
    console.log(`${name.padEnd(18)}${kumo.ms.toFixed(1).padStart(10)}${native.ms.toFixed(1).padStart(10)}` +
        `${(kumo.ms / native.ms).toFixed(0).padStart(8)}x${mismatch}`);
}
fs.rmSync(tmp, { recursive: true, force: true });
