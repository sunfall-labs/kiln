// `node:process` (and bare `process`): the global `process` object, as in Node, whose builtin
// module's default export is that object and whose named exports are its members.
//
// The named exports are Node's: the object's own enumerable members (not the inherited emitter
// methods, which Node does not export either), limited to those both engines' `process` has. A
// member Node has and kiln does not is absent here rather than exported as `undefined`:
// `import { x } from "node:process"` then fails at link time naming `x`, instead of yielding an
// `undefined` that fails three frames later (AGENTS.md, "An `undefined` from the native backend
// is the expensive answer"). kiln's own process API is `kiln:process`, never part of this
// namespace (ADR 0068).

const process: any = (globalThis as any).process;

export default process;

export const arch = process.arch;
export const argv = process.argv;
export const cwd = process.cwd;
export const env = process.env;
export const execArgv = process.execArgv;
export const execPath = process.execPath;
export const exit = process.exit;
export const features = process.features;
export const getBuiltinModule = process.getBuiltinModule;
export const hrtime = process.hrtime;
export const kill = process.kill;
export const nextTick = process.nextTick;
export const pid = process.pid;
export const platform = process.platform;
export const release = process.release;
export const stderr = process.stderr;
export const stdin = process.stdin;
export const stdout = process.stdout;
export const version = process.version;
export const versions = process.versions;
