// `IsCallable(callbackfn)` comes before the walk, so an empty receiver throws as surely as a full
// one: `[].map(1)` is a TypeError, not `[]`. Each parallel member is its sequential method and
// throws the same; node has no parallel family, so those lines use the sequential method there.
const P = typeof [].parallelMap === "function";
function attempt(label, f) {
  try { console.log(label, String(f())); } catch (e) { console.log(label, "threw", e.constructor.name); }
}
const empties = [[], [1, 2], new Array(3), new Float64Array(0), new Uint8Array([1])];
const methods = ["map", "filter", "flatMap", "forEach", "some", "every", "find", "findIndex", "findLast", "findLastIndex"];
for (const receiver of empties) {
  for (const name of methods) {
    if (typeof receiver[name] !== "function") continue;
    for (const bad of [1, undefined, null, "f", {}]) {
      attempt(`${receiver.constructor.name}(${receiver.length}).${name}(${String(bad)})`, () => receiver[name](bad));
    }
  }
}
const family = { parallelMap: "map", parallelFilter: "filter", parallelFlatMap: "flatMap" };
for (const receiver of [[], [1], new Float64Array(0)]) {
  for (const [name, seq] of Object.entries(family)) {
    const method = P ? receiver[name] : receiver[seq];
    if (typeof method !== "function") continue;
    attempt(`${receiver.constructor.name}(${receiver.length}).${seq} family`, () => method.call(receiver, 1));
  }
  const reduce = P ? receiver.parallelReduce : undefined;
  attempt(`${receiver.constructor.name}(${receiver.length}).parallelReduce`, () => {
    if (!P) throw new TypeError("1 is not a function");
    return reduce.call(receiver, 1, (a, b) => a + b, 0);
  });
}
