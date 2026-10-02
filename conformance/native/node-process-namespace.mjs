// `node:process` is Node's process module and nothing else: its default export is the global
// `process`, its named exports are that object's own members (never the inherited emitter
// methods), and kiln's own process API lives at `kiln:process`, not here (ADR 0068).
import process, { nextTick, hrtime, env, kill, cwd, exit } from "node:process";
import * as ns from "node:process";
import bare from "process";

console.log("default is the global:", process === globalThis.process, bare === globalThis.process);
console.log("named exports are its members:",
  nextTick === process.nextTick, hrtime === process.hrtime, env === process.env,
  kill === process.kill, cwd === process.cwd, exit === process.exit);
console.log("no kiln run anywhere:", "run" in process, "run" in ns);
console.log("no inherited emitter exports:", "on" in ns, "emit" in ns, "once" in ns);
const keys = Object.keys(process);
console.log("enumerable own functions:",
  ["cwd", "exit", "kill", "nextTick", "hrtime", "getBuiltinModule"].map(k => keys.includes(k)).join(","));
console.log("no internals leak:", keys.some(k => k.startsWith("__kiln")));
console.log("emitter methods are inherited:",
  ["on", "off", "once", "emit", "removeListener", "listenerCount"]
    .map(k => typeof process[k] === "function" && k in process && !Object.hasOwn(process, k)).join(","));
