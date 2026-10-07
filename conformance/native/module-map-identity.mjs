// One module map per realm: a module is evaluated once however it is reached, and every
// importer -- static or dynamic, first or later -- sees the same bindings and namespace object.
import { K, bump } from "./fixtures/module-map-identity/m.mjs";
import * as ns from "./fixtures/module-map-identity/m.mjs";
import { early } from "./fixtures/module-map-identity/early.mjs";
import { v } from "./fixtures/module-map-identity/late.mjs";
import * as late from "./fixtures/module-map-identity/late.mjs";
import { get as gx } from "./fixtures/module-map-identity/x.mjs";
import { get as gy } from "./fixtures/module-map-identity/y.mjs";
import { EventEmitter } from "node:events";
import * as events from "node:events";
import * as inner from "./fixtures/module-map-identity/ns-inner.mjs";
import * as barrel from "./fixtures/module-map-identity/ns-barrel.mjs";

const d = await import("./fixtures/module-map-identity/m.mjs");
const again = await import("./fixtures/module-map-identity/m.mjs");
bump();
console.log(
  "static-then-dynamic:",
  K === d.K,
  ns === d,
  d === again,
  d.n === 2,
  globalThis.__mRuns === 1,
);
console.log("dynamic-then-static:", early.v === v, early === late, globalThis.__lateRuns === 1);

const [p, q] = await Promise.all([gx(), gy()]);
console.log("two-importers:", p === q, p.o === q.o, globalThis.__sharedRuns === 1);

const ev = await import("node:events");
console.log("builtin:", EventEmitter === ev.EventEmitter, events === ev);

let first;
let second;
try {
  await import("./fixtures/module-map-identity/bad.mjs");
} catch (error) {
  first = error;
}
try {
  await import("./fixtures/module-map-identity/bad.mjs");
} catch (error) {
  second = error;
}
console.log("failed:", first === second, first instanceof Error, globalThis.__badRuns === 1);

// `export * as` re-exports the module's own namespace object, statically and dynamically, and a
// module reached only by `import()` still has its `export * as` names.
const dynBarrel = await import("./fixtures/module-map-identity/ns-barrel.mjs");
console.log(
  "export-star-as:",
  barrel.inner === inner,
  dynBarrel === barrel,
  dynBarrel.inner === inner,
  Object.keys(dynBarrel).join(","),
);
const onlyDynamic = await import("./fixtures/module-map-identity/dyn-barrel.mjs");
const dynInner = await import("./fixtures/module-map-identity/dyn-inner.mjs");
console.log(
  "dynamic-star-as:",
  Object.keys(onlyDynamic).join(","),
  "nested" in onlyDynamic,
  onlyDynamic.nested === dynInner,
  onlyDynamic.nested.c,
);
