// Test262 subset runner. Usage:
//   node tests/test262/run.js [path-filter ...] [--jobs N] [--update] [--failures]
// Needs a Test262 checkout in target/test262 (see FETCH below). Runs test/language minus
// what kumo doesn't support (modules, `with`, sloppy mode, direct eval, other realms, ...),
// compiling everything with one `kumoc --batch` and running it on long-lived worker
// threads, each test in a fresh context. Prints pass rates per directory and compares
// them with tests/test262/summary.txt (`--update` rewrites it; `--failures` lists them).
'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');
const nodeVm = require('vm');
const { execFileSync } = require('child_process');
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');

const ROOT = path.join(__dirname, '../..');
const T262 = path.join(ROOT, 'target/test262');
const WORK = path.join(ROOT, 'target/test262-run');
const SUMMARY = path.join(__dirname, 'summary.txt');
const FETCH = 'git clone --depth 1 --filter=blob:none --sparse https://github.com/tc39/test262.git target/test262 ' +
    '&& git -C target/test262 sparse-checkout set harness test/language';
const TIMEOUT_MS = 2000;

// not supported by kumo (by design or not yet), so not run
const SKIP_DIRS = ['module-code', 'import', 'export', 'eval-code', 'statements/with', 'expressions/dynamic-import',
    'expressions/import.meta'];
const SKIP_FLAGS = ['module', 'noStrict', 'raw', 'CanBlockIsFalse', 'CanBlockIsTrue'];
const SKIP_FEATURES = ['import-defer', 'source-phase-imports', 'decorators', 'explicit-resource-management',
    'json-modules', 'import-attributes', 'import-assertions', 'dynamic-import', 'import.meta', 'top-level-await',
    'tail-call-optimization', 'cross-realm', 'IsHTMLDDA', 'host-gc-required', 'ShadowRealm', 'Atomics.waitAsync'];
// names compiled specially, or that can't be declared in strict mode
const NOT_DECLARED = new Set(['undefined', 'NaN', 'Infinity', 'globalThis', 'window', 'eval', 'arguments']);

// ---------------------------------------------------------------------------------------
// worker: runs compiled tests, one fresh context each
// ---------------------------------------------------------------------------------------
if (!isMainThread) {
    const vmScript = new nodeVm.Script(fs.readFileSync(path.join(ROOT, 'src/vm/vm.js'), 'utf8') +
        '\n;globalThis.__kumoVM = VM;');
    const settle = () => new Promise((r) => setImmediate(r));

    async function runTest(test) {
        const prints = [];
        const context = nodeVm.createContext({});
        context.print = (msg) => prints.push(String(msg));
        nodeVm.runInContext(`globalThis.$262 = {
            global: globalThis,
            evalScript: (src) => (0, eval)(src),
            detachArrayBuffer: (buffer) => { structuredClone(buffer, { transfer: [buffer] }); },
            gc: undefined,
            agent: {},
        };`, context);
        vmScript.runInContext(context);
        context.__bytecode = new Uint8Array(fs.readFileSync(test.bytecode));
        let error = null;
        try {
            nodeVm.runInContext('new __kumoVM([__bytecode]).run();', context, { timeout: TIMEOUT_MS });
        } catch (e) {
            error = e;
        }
        if (test.async && !error) {
            const deadline = Date.now() + TIMEOUT_MS;
            while (!prints.some((p) => p.startsWith('Test262:AsyncTest')) && Date.now() < deadline) await settle();
        }
        const errorName = error && ((error.constructor && error.constructor.name) || error.name || String(error));
        if (test.negative) {
            if (error && errorName === test.negative.type) return { pass: true };
            return { pass: false, why: error ? `threw ${errorName}, expected ${test.negative.type}` : `expected ${test.negative.type}` };
        }
        if (error) {
            const message = error && error.message ? `: ${error.message}` : '';
            return { pass: false, why: `threw ${errorName}${message}`.slice(0, 300) };
        }
        if (test.async) {
            const done = prints.find((p) => p.startsWith('Test262:AsyncTest'));
            if (done === 'Test262:AsyncTestComplete') return { pass: true };
            return { pass: false, why: done ? done.slice(0, 300) : 'async test timed out' };
        }
        return { pass: true };
    }

    parentPort.on('message', async (test) => {
        let result;
        try {
            result = await runTest(test);
        } catch (e) {
            result = { pass: false, why: `runner error: ${e && e.message}` };
        }
        parentPort.postMessage({ id: test.id, ...result });
    });
    return;
}

// ---------------------------------------------------------------------------------------
// main: select, prepare, compile, run, report
// ---------------------------------------------------------------------------------------
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const jobsAt = args.indexOf('--jobs');
const jobs = jobsAt >= 0 ? +args[jobsAt + 1] : os.cpus().length;
const filters = args.filter((a, i) => !a.startsWith('--') && args[i - 1] !== '--jobs');

if (!fs.existsSync(path.join(T262, 'harness'))) {
    console.error(`Test262 not found in ${T262}. Fetch it with:\n  ${FETCH}`);
    process.exit(1);
}

// YAML frontmatter: just the keys used here (lists inline `[a, b]` or as `- a` lines)
function frontmatter(source) {
    const m = source.match(/\/\*---([\s\S]*?)---\*\//);
    const meta = { includes: [], flags: [], features: [], negative: null };
    if (!m) return meta;
    let key = null;
    for (const line of m[1].split('\n')) {
        const top = line.match(/^(\w+):\s*(.*)$/);
        if (top) {
            key = top[1];
            const value = top[2].trim();
            if (value.startsWith('[')) {
                meta[key] = value.slice(1, -1).split(',').map((s) => s.trim()).filter(Boolean);
            } else if (key === 'negative') {
                meta.negative = {};
            }
            continue;
        }
        const item = line.match(/^\s+-\s+(.*)$/);
        if (item && Array.isArray(meta[key])) meta[key].push(item[1].trim());
        const sub = line.match(/^\s+(phase|type):\s*(\S+)/);
        if (sub && key === 'negative') meta.negative[sub[1]] = sub[2];
    }
    return meta;
}

function listTests(dir, out) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) listTests(full, out);
        else if (entry.name.endsWith('.js') && !entry.name.includes('_FIXTURE')) out.push(full);
    }
    return out;
}

const languageDir = path.join(T262, 'test/language');
const harnessCache = new Map();
const harness = (name) => {
    if (!harnessCache.has(name)) harnessCache.set(name, fs.readFileSync(path.join(T262, 'harness', name), 'utf8'));
    return harnessCache.get(name);
};
const contextGlobals = new Set(nodeVm.runInContext('Object.getOwnPropertyNames(globalThis)', nodeVm.createContext({})));
['print', '$262'].forEach((n) => contextGlobals.add(n));

fs.rmSync(WORK, { recursive: true, force: true });
fs.mkdirSync(path.join(WORK, 'src'), { recursive: true });
fs.mkdirSync(path.join(WORK, 'out'), { recursive: true });

const tests = [];
let skipped = 0;
for (const file of listTests(languageDir, []).sort()) {
    const rel = path.relative(languageDir, file);
    if (filters.length && !filters.some((f) => rel.includes(f))) continue;
    const source = fs.readFileSync(file, 'utf8');
    const meta = frontmatter(source);
    if (SKIP_DIRS.some((d) => rel.startsWith(d + path.sep)) ||
        meta.flags.some((f) => SKIP_FLAGS.includes(f)) ||
        meta.features.some((f) => SKIP_FEATURES.includes(f)) ||
        /\$262\.(createRealm|agent|IsHTMLDDA)/.test(source)) {
        skipped++;
        continue;
    }
    const isAsync = meta.flags.includes('async');
    const includes = ['assert.js', 'sta.js', ...(isAsync ? ['doneprintHandle.js'] : []), ...meta.includes];
    // harness functions are VM locals in kumo; asyncHelpers looks for $DONE on the global
    const glue = isAsync ? '\nglobalThis.$DONE = $DONE;\n' : '\n';
    const body = includes.map(harness).join('\n') + glue + source;
    // kumo has no implicit globals: declare the host globals the code uses
    const used = new Set(body.match(/[A-Za-z_$][\w$]*/g));
    const declared = [...used].filter((name) => contextGlobals.has(name) && !NOT_DECLARED.has(name) &&
        !new RegExp(`\\b(var|let|const|function\\*?|class)\\s+${name.replace(/\$/g, '\\$')}\\b`).test(body));
    const prelude = declared.map((name) => `var ${name} = globalThis.${name};`).join('\n');
    const id = tests.length;
    const prepared = path.join(WORK, 'src', `${id}.js`);
    fs.writeFileSync(prepared, prelude + '\n' + body);
    tests.push({ id, rel, prepared, async: isAsync, negative: meta.negative });
}

console.log(`compiling ${tests.length} tests (${skipped} skipped as unsupported)...`);
fs.writeFileSync(path.join(WORK, 'list.txt'), tests.map((t) => t.prepared).join('\n'));
execFileSync('cargo', ['build', '--release', '--quiet', '--bin', 'kumoc'], { cwd: ROOT, stdio: 'inherit' });
execFileSync(path.join(ROOT, 'target/release/kumoc'), ['--batch', path.join(WORK, 'list.txt'), path.join(WORK, 'out')]);

const results = new Array(tests.length);
const toRun = [];
for (const test of tests) {
    const err = path.join(WORK, 'out', `${test.id}.err`);
    if (fs.existsSync(err)) {
        // a parse-time negative test passes by not compiling
        const early = test.negative && test.negative.phase !== 'runtime';
        const message = fs.readFileSync(err, 'utf8').replace(/ at \S+$/, '');
        // kumo rejects undeclared names (no implicit globals) and has no direct eval
        const design = /undeclared variable `/.test(message);
        results[test.id] = early ? { pass: true } : { pass: false, design, why: 'compile: ' + message };
    } else if (test.negative && test.negative.phase !== 'runtime') {
        results[test.id] = { pass: false, why: `compiled, expected early ${test.negative.type}` };
    } else {
        toRun.push({ id: test.id, bytecode: path.join(WORK, 'out', `${test.id}.kumo`), async: test.async, negative: test.negative });
    }
}

console.log(`running ${toRun.length} on ${jobs} workers...`);
const started = Date.now();
(async () => {
    await new Promise((resolveAll) => {
        let next = 0;
        let pending = toRun.length;
        if (pending === 0) resolveAll();
        const spawn = () => {
            const worker = new Worker(__filename, { workerData: {} });
            let current = null;
            const feed = () => {
                if (next >= toRun.length) {
                    worker.terminate();
                    return;
                }
                current = toRun[next++];
                worker.postMessage(current);
            };
            worker.on('message', (r) => {
                results[r.id] = { pass: r.pass, why: r.why };
                current = null;
                if (--pending === 0) resolveAll();
                feed();
            });
            // a crashed worker fails its test and is replaced
            worker.on('error', (e) => {
                if (current) {
                    results[current.id] = { pass: false, why: `worker crashed: ${e && e.message}` };
                    if (--pending === 0) resolveAll();
                }
                if (next < toRun.length) spawn();
            });
            feed();
        };
        for (let i = 0; i < Math.min(jobs, toRun.length); i++) spawn();
    });

    // pass rates per directory (up to two levels under test/language)
    const groups = new Map();
    let passed = 0;
    const design = tests.filter((t) => results[t.id].design).length;
    for (const test of tests) {
        if (results[test.id].design) continue;
        const parts = test.rel.split(path.sep);
        const group = parts.slice(0, parts.length > 2 ? 2 : 1).join('/');
        const g = groups.get(group) || { pass: 0, total: 0 };
        g.total++;
        if (results[test.id].pass) {
            g.pass++;
            passed++;
        }
        groups.set(group, g);
    }
    const lines = [...groups].sort().map(([group, g]) => `${group} ${g.pass}/${g.total}`);
    const counted = tests.length - design;
    const total = `total ${passed}/${counted} (${(100 * passed / counted).toFixed(1)}%), ` +
        `${design} more rejected by design (undeclared names, direct eval)`;
    console.log(`done in ${((Date.now() - started) / 1000).toFixed(1)}s\n${total}`);

    fs.writeFileSync(path.join(WORK, 'results.json'), JSON.stringify(
        tests.map((t) => ({ test: t.rel, ...results[t.id] })), null, 1));
    if (flag('--failures')) {
        for (const t of tests) {
            if (!results[t.id].pass && !results[t.id].design) console.log(`FAIL ${t.rel}: ${results[t.id].why}`);
        }
    }

    // compare with the recorded summary (only meaningful for a full, unfiltered run)
    if (filters.length === 0) {
        const old = fs.existsSync(SUMMARY) ? fs.readFileSync(SUMMARY, 'utf8').split('\n') : [];
        const oldCounts = new Map(old.map((l) => l.split(' ')).filter((p) => p.length === 2).map(([g, c]) => [g, c]));
        for (const line of lines) {
            const [group, count] = line.split(' ');
            const before = oldCounts.get(group);
            if (before && before !== count) console.log(`changed: ${group} ${before} -> ${count}`);
        }
        if (flag('--update')) {
            fs.writeFileSync(SUMMARY, `# node tests/test262/run.js --update (test262 ${execFileSync('git', ['-C', T262, 'rev-parse', '--short', 'HEAD']).toString().trim()})\n${total}\n${lines.join('\n')}\n`);
            console.log(`wrote ${path.relative(ROOT, SUMMARY)}`);
        }
    }
    console.log(`details: ${path.relative(ROOT, path.join(WORK, 'results.json'))}`);
})();
