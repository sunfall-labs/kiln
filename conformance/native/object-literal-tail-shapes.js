// kiln-native: profile=native-release
// Cached literal shapes also cover GC-owned tail slots. Keep references alive across allocations,
// change numeric/non-numeric layouts at one site, then mutate only one instance's descriptors.
function state(value) {
  return {
    buffer: [value], length: value, pipes: [], flowing: null,
    ended: false, endEmitted: false, reading: false, destroyed: false,
    highWaterMark: 16384, encoding: null, objectMode: false,
  };
}
function maximum(value) {
  return {
    k0: value, k1: 1, k2: 2, k3: 3, k4: 4, k5: 5, k6: 6, k7: 7,
    k8: 8, k9: 9, k10: 10, k11: 11, k12: 12, k13: 13, k14: 14, k15: 15,
    k16: 16, k17: 17, k18: 18, k19: 19, k20: 20, k21: 21, k22: 22, k23: 23,
    k24: 24, k25: 25, k26: 26, k27: 27, k28: 28, k29: 29, k30: 30, k31: value,
  };
}
const kept = [];
for (let index = 0; index < 24; index++) {
  const value = index % 2 ? "s" + index : index;
  kept.push(state(value), maximum(value));
}
for (let index = 0; index < 24; index++) {
  const left = kept[index * 2], right = kept[index * 2 + 1];
  const value = index % 2 ? "s" + index : index;
  if (left.buffer[0] !== value || left.length !== value || left.pipes.length !== 0 ||
      Object.keys(left).length !== 11 || right.k0 !== value || right.k31 !== value ||
      Object.keys(right).length !== 32) throw new Error("cached tail slots lost a value");
}
const changed = kept[0];
Object.defineProperty(changed, "length", { get() { return 99; }, configurable: true });
delete changed.encoding;
changed.extra = "changed";
const fresh = state(42);
console.log("tail references", kept.length, changed.length, fresh.length, fresh.buffer[0]);
console.log("tail descriptors", Object.keys(changed).join(","), JSON.stringify(fresh));
console.log("tail boundary", maximum("end").k31, Object.getPrototypeOf(fresh) === Object.prototype);
