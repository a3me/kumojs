// Strips the types from src/vm/vm.ts and writes src/vm/vm.js (generated, gitignored).
import { stripTypeScriptTypes } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';

const dir = new URL('../src/vm/', import.meta.url);
const out = stripTypeScriptTypes(readFileSync(new URL('vm.ts', dir), 'utf8'), { mode: 'strip' });
const target = new URL('vm.js', dir);
let previous = null;
try { previous = readFileSync(target, 'utf8'); } catch {}
if (previous !== out) writeFileSync(target, out); // untouched when unchanged, so no needless rebuilds
