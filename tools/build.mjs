// The kumo build: turns src/vm/vm.ts into runnable JS (optionally with its opcodes
// renumbered), and bundles a compiled program with a matching VM.
//
//   node tools/build.mjs vm [--out <vm.js>] [--seed N | --shuffle] [--map <map.json>]
//       Build the VM (default: the canonical src/vm/vm.js, as build.rs does). `--map`
//       writes the opcode numbering for `kumoc --opcode-map`.
//   node tools/build.mjs bundle <input.js> [--out <out.js>] [--seed N | --canonical]
//       Compile <input.js> with kumoc and write a standalone script: the VM plus the
//       program, run on load. Opcodes are shuffled to a random seed unless pinned.
//
// vm.ts refers to opcodes only as `Op.Name` (declared in src/vm/opcodes.d.ts, generated
// from the Rust opcode table). The build runs a list of passes over the type-stripped
// source; new build options (e.g. shuffling handler order) are new passes.
// `KUMOC` overrides the kumoc command used by `bundle` (default: `cargo run -q --bin kumoc --`).

import { stripTypeScriptTypes } from 'node:module';
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { randomInt } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const VM_TS = join(root, 'src/vm/vm.ts');
const OPCODES_D_TS = join(root, 'src/vm/opcodes.d.ts');
const CANONICAL_VM = join(root, 'src/vm/vm.js');

// ---- opcode numbering

/** The canonical opcode bytes, by name, from opcodes.d.ts. */
export function canonicalOpcodes() {
    const opcodes = new Map();
    const declaration = /^\s*readonly (\w+): (0x[0-9a-f]{2});$/gm;
    for (const [, name, byte] of readFileSync(OPCODES_D_TS, 'utf8').matchAll(declaration)) {
        opcodes.set(name, Number(byte));
    }
    if (opcodes.size === 0) throw new Error(`no opcodes declared in ${OPCODES_D_TS}`);
    return opcodes;
}

/** The opcode bytes shuffled among themselves; the same seed always gives the same map. */
export function shuffledOpcodes(seed) {
    const canonical = canonicalOpcodes();
    const bytes = [...canonical.values()].sort((a, b) => a - b);
    const random = mulberry32(seed);
    for (let i = bytes.length - 1; i > 0; i--) {
        const j = Math.floor(random() * (i + 1));
        [bytes[i], bytes[j]] = [bytes[j], bytes[i]];
    }
    const names = [...canonical.entries()].sort((a, b) => a[1] - b[1]).map(([name]) => name);
    return new Map(names.map((name, i) => [name, bytes[i]]));
}

/** A small deterministic PRNG: a 32-bit seed to floats in [0, 1). */
function mulberry32(seed) {
    let a = seed >>> 0;
    return () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

/** The numbering as `kumoc --opcode-map` reads it: `{ "LoadString": 23, ... }`. */
export function opcodeMapJson(opcodes) {
    return JSON.stringify(Object.fromEntries(opcodes), null, 2) + '\n';
}

// ---- the VM

/**
 * The VM as runnable JS. `opcodes` (name -> byte) numbers the opcodes; `passes` run in
 * order on the type-stripped source after the opcodes are inlined.
 */
export function buildVm({ opcodes = canonicalOpcodes(), passes = [] } = {}) {
    let source = stripTypeScriptTypes(readFileSync(VM_TS, 'utf8'), { mode: 'strip' });
    for (const pass of [inlineOpcodes(opcodes), ...passes]) {
        source = pass(source);
    }
    return source;
}

/**
 * Replace every `Op.Name` with its byte. Every opcode must have exactly one handler
 * (`case Op.Name:`), so the compiler and the VM can't disagree about what bytes exist.
 */
function inlineOpcodes(opcodes) {
    return (source) => {
        const handled = new Map();
        for (const [, name] of source.matchAll(/case Op\.(\w+):/g)) {
            handled.set(name, (handled.get(name) ?? 0) + 1);
        }
        const problems = [];
        for (const name of opcodes.keys()) {
            if (handled.get(name) !== 1) problems.push(`${name}: ${handled.get(name) ?? 0} handlers`);
        }
        for (const name of handled.keys()) {
            if (!opcodes.has(name)) problems.push(`${name}: not an opcode`);
        }
        if (problems.length) throw new Error(`vm.ts opcode handlers:\n  ${problems.join('\n  ')}`);

        return source.replace(/\bOp\.(\w+)\b/g, (_, name) => {
            const byte = opcodes.get(name);
            if (byte === undefined) throw new Error(`vm.ts uses unknown opcode Op.${name}`);
            return `0x${byte.toString(16).padStart(2, '0')}`;
        });
    };
}

// ---- bundling a program

/** Compile `input` under `opcodes` with kumoc; returns the bytecode. */
export function compileProgram(input, opcodes) {
    const dir = mkdtempSync(join(tmpdir(), 'kumo-build-'));
    try {
        const mapPath = join(dir, 'opcodes.json');
        const outPath = join(dir, 'program.kumo');
        writeFileSync(mapPath, opcodeMapJson(opcodes));
        const [command, ...prefix] = process.env.KUMOC
            ? [process.env.KUMOC]
            : ['cargo', 'run', '-q', '--bin', 'kumoc', '--'];
        execFileSync(command, [...prefix, '--opcode-map', mapPath, input, outPath], {
            cwd: root,
            stdio: ['ignore', 'inherit', 'inherit'],
        });
        return readFileSync(outPath);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

/** A standalone script: the VM for `opcodes` plus `bytecode`, run on load. */
export function bundle(bytecode, opcodes, options = {}) {
    return buildVm({ opcodes, ...options }) +
        '\n;(function () {\n' +
        `  var code = [${Array.from(bytecode).join(',')}];\n` +
        '  new VM([code]).run();\n' +
        '})();\n';
}

// ---- command line

function writeIfChanged(path, text) {
    let previous = null;
    try { previous = readFileSync(path, 'utf8'); } catch {}
    // left untouched when unchanged, so build.rs doesn't trigger needless rebuilds
    if (previous !== text) writeFileSync(path, text);
}

function parseSeed(value) {
    const seed = Number(value);
    if (!Number.isInteger(seed) || seed < 0 || seed >= 2 ** 32) {
        throw new Error(`--seed takes an integer from 0 to 2^32-1, got ${value}`);
    }
    return seed;
}

function parseArgs(args) {
    const options = { positional: [] };
    for (let i = 0; i < args.length; i++) {
        const arg = args[i];
        const value = () => {
            if (i + 1 >= args.length) throw new Error(`${arg} needs a value`);
            return args[++i];
        };
        switch (arg) {
            case '--out': options.out = value(); break;
            case '--map': options.map = value(); break;
            case '--seed': options.seed = parseSeed(value()); break;
            case '--shuffle': options.shuffle = true; break;
            case '--canonical': options.canonical = true; break;
            default:
                if (arg.startsWith('--')) throw new Error(`unknown option ${arg}`);
                options.positional.push(arg);
        }
    }
    return options;
}

/** The numbering to build with: canonical, or shuffled (to a random seed, reported). */
function chooseOpcodes({ seed, shuffle }) {
    if (seed === undefined && !shuffle) return canonicalOpcodes();
    if (seed === undefined) {
        seed = randomInt(2 ** 32);
        console.error(`opcode seed ${seed}`);
    }
    return shuffledOpcodes(seed);
}

const USAGE = `usage: node tools/build.mjs vm [--out <vm.js>] [--seed N | --shuffle] [--map <map.json>]
       node tools/build.mjs bundle <input.js> [--out <out.js>] [--seed N | --canonical]`;

function main([command, ...args]) {
    const options = parseArgs(args);
    switch (command) {
        case 'vm': {
            if (options.positional.length) throw new Error(USAGE);
            const opcodes = chooseOpcodes(options);
            writeIfChanged(options.out ?? CANONICAL_VM, buildVm({ opcodes }));
            if (options.map) writeFileSync(options.map, opcodeMapJson(opcodes));
            break;
        }
        case 'bundle': {
            if (options.positional.length !== 1) throw new Error(USAGE);
            const opcodes = chooseOpcodes({ ...options, shuffle: !options.canonical });
            const script = bundle(compileProgram(options.positional[0], opcodes), opcodes);
            if (options.out) writeFileSync(options.out, script);
            else process.stdout.write(script);
            break;
        }
        default:
            throw new Error(USAGE);
    }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    try {
        main(process.argv.slice(2));
    } catch (e) {
        console.error(e.message);
        process.exit(1);
    }
}
