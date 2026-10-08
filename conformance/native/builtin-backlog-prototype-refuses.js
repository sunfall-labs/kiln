// Atomics.add is a known native backlog member while shared memory has no representation. A
// replacement parent must not mask that unresolved own member with an inherited Proxy value.
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
if (typeof Atomics.add !== "function") throw new Error("Node has its own Atomics.add");
if (reads.get !== 0 || reads.has !== 0) throw new Error("Node consulted Atomics' replacement parent");
console.log("Node keeps the own Atomics.add member ahead of its replacement parent");
const unexpected = Atomics.add;
if (unexpected !== Atomics.add) throw new Error("Node returned an unstable own member");
if (reads.get !== 0 || reads.has !== 0) throw new Error("Node consulted Atomics' replacement parent");
