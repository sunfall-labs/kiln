import { bench } from "../harness.mjs";

// node and bun have no `parallelMap`; kiln does, on both engines. The polyfill is keyed on the
// host's identity rather than on `typeof`, because the native backend does not yet describe
// `parallelMap` as an array member -- a `typeof` read answers `undefined` there while the
// interpreter answers `function` (filed in .scratch/rust-parallel-engine/issues/06) -- and the
// package's `defineProperty` on the prototype is refused by the native backend anyway.
if (
  (typeof process !== "undefined" && process.release && process.release.name === "node") ||
  typeof Bun !== "undefined"
) {
  Array.prototype.parallelMap = function (fn) {
    return this.map(fn);
  };
}

// Two million elements and a body heavy enough for a fork-join to pay: a square root, a power,
// a remainder and a floor per element. On kiln's native backend the callback qualifies for the
// provider's pool (ADR 0058, application 2), and `KILN_PARALLELISM=1` runs the same binary
// inline for the sequential answer. The checksum is a sum in index order, so every runtime must
// agree on it.
const numbers = [];
for (let i = 0; i < 2000000; i++) numbers.push(i * 0.5);

bench("parallel-map-large", () => {
  const mapped = numbers.parallelMap(x => Math.sqrt(x * x + 1) * Math.pow(x % 7 + 1, 1.5) + Math.floor(x / 3));
  let total = 0;
  for (let i = 0; i < mapped.length; i++) total += mapped[i];
  return total;
});
