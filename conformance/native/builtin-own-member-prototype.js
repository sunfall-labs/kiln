// A builtin's own property must win over its replacement prototype, even when the parent is a
// Proxy whose `get` would return a value for the same name.
const reads = { get: 0, has: 0 };
const parent = new Proxy({}, {
  get(_target, _key, receiver) {
    reads.get += 1;
    return receiver;
  },
  has() {
    reads.has += 1;
    return false;
  },
});
Object.setPrototypeOf(Atomics, parent);
if (typeof Atomics.add !== "function") throw new Error("Atomics.add must resolve from the builtin itself");
if (reads.get !== 0 || reads.has !== 0) throw new Error("the read consulted Atomics' replacement parent");
console.log("builtin own member wins over replacement parent");
