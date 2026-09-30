const captured = 41;
console.log(eval("captured + 1"));
console.log(eval("require('util').inspect")({ value: captured }));

function shadowed(eval) {
  return eval("unchanged");
}
console.log(shadowed((source) => `shadow:${source}`));

const intrinsicEval = eval;
let reads = 0;
Object.defineProperty(globalThis, "eval", {
  configurable: true,
  get() {
    reads++;
    return intrinsicEval;
  },
});
console.log(eval("captured + 2"), reads);
Object.defineProperty(globalThis, "eval", {
  configurable: true,
  writable: true,
  value(source) {
    "use strict";
    return `${this === undefined}:${source}`;
  },
});
console.log(eval("captured + 3"));
globalThis.eval = intrinsicEval;
