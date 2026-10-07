import { setFlagsFromString } from "node:v8";
import { runInNewContext } from "node:vm";

if (runInNewContext("seed + 2", { seed: 40 }) !== 42) {
  throw new Error("new-context scope ignored its context bindings");
}
const context = { state: { count: 0 } };
if (runInNewContext("state.count += 1", context) !== 1 || context.state.count !== 1) {
  throw new Error("new-context scope lost assignment side effects");
}

if (typeof globalThis.gc !== "undefined") {
  throw new Error("GC exposure leaked onto the current global");
}

setFlagsFromString("--expose_gc");
const collect = runInNewContext("gc");
if (typeof collect !== "function") {
  throw new Error("new context did not receive gc");
}
setFlagsFromString("--no-expose_gc");
if (runInNewContext("typeof gc") !== "undefined") {
  throw new Error("new contexts kept exposing gc after the flag was cleared");
}
if (typeof globalThis.gc !== "undefined") {
  throw new Error("GC exposure changed the current global");
}

let reference;
(() => {
  const target = {};
  reference = new WeakRef(target);
})();

let turns = 0;
const collectAcrossTurns = () => {
  setTimeout(() => {
    collect();
    turns += 1;
    if (reference.deref() === undefined) {
      console.log("deferred collection passed");
      return;
    }
    if (turns === 8) throw new Error("requested collection did not clear the weak target");
    collectAcrossTurns();
  }, 0);
};
collectAcrossTurns();
