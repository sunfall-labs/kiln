// kiln-native: profile=native-release
//
// A method that reads `this.x` keeps a receiver-shape cache: rows of shapes it has seen, each
// with the slot of every name it reads. An object whose payload is not a shaped object -- a
// `Date` -- reads as having no shape, and an unclaimed row's key is no shape too, so it matched
// the first empty row and read `slots[UINT32_MAX]`. `Date.prototype[Symbol.toPrimitive]` does
// exactly this read (`this.valueOf`), so every `d < e`, `${d}` and `d == n` of a release build
// with the specialized bootstrap crashed (.scratch/parallel-builtins issue 18).
const read = { m() { return typeof this.valueOf + " " + typeof this.toString; } }.m;
console.log(read.call(new Date(1)), read.call({ valueOf() {}, toString: 1 }), read.call(new Date(2)));

const d = new Date(86400000);
const e = new Date(172800000);
console.log(d < e, d <= e, d > e, `${d}` === String(d), d == 86400000, d - e, [d].join("") === String(d));
