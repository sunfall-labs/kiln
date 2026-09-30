// kiln eval smoke test (authored for kiln, not from porffor).
//
// upm's CLI entry (dist/upm.mjs) starts up via
// process.getBuiltinModule('node:os' | 'node:path' | 'node:module' | 'node:fs').
// We verify those four builtins resolve through the same API, then run upm's
// real CLI `--help` through its exported start() for deterministic output.
import { start } from '../node_modules/upm/dist/_chunks/cli.mjs';

const e = globalThis.process;
const os = e.getBuiltinModule('node:os');
const path = e.getBuiltinModule('node:path');
const mod = e.getBuiltinModule('node:module');
const fs = e.getBuiltinModule('node:fs');
console.log(typeof os.homedir());
console.log(path.join('a', 'b'));
console.log(typeof mod.createRequire);
console.log(typeof mod.enableCompileCache);
console.log(typeof fs.realpathSync);

await start(['--help']);
