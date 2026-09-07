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


const numbers = [];
for (let i = 0; i < 5000; i++) {
  numbers.push(i);
}

bench("multicore-parallel", () => {
  let total = 0;
  for (let iter = 0; iter < 10; iter++) {
    const mapped = numbers.parallelMap(x => x * 2 + 1);
    const result = mapped
      .filter(x => x % 3 === 0)
      .reduce((acc, x) => acc + x, 0);
    total += result;
  }
  return total;
});
