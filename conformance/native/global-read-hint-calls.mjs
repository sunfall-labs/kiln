// Many reads of global singletons across nested and recursive calls, with the global rebound
// part way through: the cached realm slots and per-site hints must not go stale.
let count = 0;
function depth(n) {
  if (n === 0) return globalThis === globalThis && typeof Math.floor === "function" ? 1 : 0;
  count += Math.floor(1.5);
  return depth(n - 1) + Math.min(1, n);
}
console.log("deep", depth(50), count);

const results = [];
for (let round = 0; round < 3; round++) {
  const saved = globalThis.Math;
  if (round === 1) globalThis.Math = { floor: (x) => x * 10, min: (a, b) => a + b };
  results.push(depth(3), count);
  globalThis.Math = saved;
}
console.log("rounds", results.join(","));

// globalThis identity is stable across reads, and `this` in a sloppy function is the same object.
function sloppy() {
  return this === globalThis;
}
console.log("identity", globalThis === globalThis.globalThis, sloppy.call(undefined) || "module-strict");
console.log("process", typeof process, process === globalThis.process, typeof console.log);

const seen = new Set();
for (let i = 0; i < 100; i++) seen.add(globalThis);
console.log("one global", seen.size);
