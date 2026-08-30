// A view is a window onto bytes, not an array of numbers: the width is fixed, the length is
// fixed, and two views over one buffer see each other's writes.
const u = new Uint8Array(4);
u[0] = 1;
u[1] = 255;
u[2] = 256;
u[3] = -1;
console.log(u.length, u[0], u[1], u[2], u[3], u[9]);
console.log(u.byteLength, u.BYTES_PER_ELEMENT, u.byteOffset);
console.log(String(u), JSON.stringify(u));
console.log(u);

// Four ways to make one: a length, a list, another view, and a buffer.
console.log(new Uint8Array([10, 20, 30]).join(","));
console.log(new Uint8Array(new Uint8Array([1, 2])).join(","));
console.log(Uint8Array.of(3).join(","), new Uint8Array(3).join(","));
console.log(Uint8Array.from([1, 2], (x) => x * 3).join(","));

// A shared buffer is the point.
const buffer = new ArrayBuffer(8);
const bytes = new Uint8Array(buffer);
const words = new Uint32Array(buffer);
bytes[0] = 1;
console.log(buffer.byteLength, bytes.length, words.length, words[0]);
words[1] = 0x01020304;
console.log(bytes[4], bytes[5], bytes[6], bytes[7]);

// `subarray` shares; `slice` copies. Getting that backwards would hide a write.
const four = new Uint8Array([1, 2, 3, 4]);
const shared = four.subarray(1, 3);
const copied = four.slice(1, 3);
shared[0] = 99;
copied[1] = 88;
console.log(four.join(","), shared.join(","), copied.join(","));

// Each element type converts differently: integers wrap, clamped saturates, floats round.
console.log(new Int8Array([200])[0], new Int16Array([70000])[0], new Uint32Array([-1])[0]);
console.log(new Uint8ClampedArray([300, -5, 1.5, 2.5])[0], new Uint8ClampedArray([300])[0]);
console.log(new Float32Array([1.1])[0], new Float64Array([1.1])[0]);

// The read-only array methods work; the ones that change the length do not exist.
const list = new Uint8Array([1, 2, 3, 4]);
console.log(list.indexOf(3), list.includes(9), list.filter((x) => x > 2).join(","));
console.log(list.reduce((a, b) => a + b, 0), list.some((x) => x === 2), list.every((x) => x > 0));
console.log(typeof list.push, typeof list.pop, Array.isArray(list), typeof list);
console.log(Symbol.iterator in list, typeof list[Symbol.iterator]);
console.log(Array.from(list).join(","), [...list].join("-"));
for (const value of new Uint8Array([7, 8])) console.log("v", value);
const liveValues = list.values();
list[0] = 9;
console.log(liveValues.next().value);
list[0] = 1;
const liveEntries = list.entries();
list[0] = 8;
console.log(liveEntries.next().value.join(","));
list[0] = 1;

// `set` and `fill` write in place.
const target = new Uint8Array(4);
target.set([5, 6], 1);
console.log(target.join(","));
target.fill(7, 1, 3);
console.log(target.join(","));

// Identity and instanceof.
console.log(list instanceof Uint8Array, list instanceof Int8Array, buffer instanceof ArrayBuffer);
console.log(list === list, new Uint8Array(1) === new Uint8Array(1));
console.log(Object.prototype.toString.call(list), list.constructor === Uint8Array);

// A method that builds a new array from a view builds a *view* of the same kind, and one that
// rearranges in place answers with the view it changed.
const kinds = new Uint8Array([3, 1, 2]);
console.log(kinds.map((x) => x * 2), kinds.filter((x) => x > 1), kinds.slice(0, 2));
console.log(new Int16Array([1, 2]).map((x) => x + 1000));
const sorted = new Uint8Array([3, 1, 2]);
console.log(sorted.sort(), sorted.join(","));
const reversed = new Uint8Array([1, 2, 3]);
console.log(reversed.reverse(), reversed.join(","));

// The same methods on a plain array still produce plain arrays.
console.log([3, 1, 2].sort(), [1, 2].map((x) => x), [1, 2].reverse(), [1, 2, 3].slice(1));

function caughtName(operation) {
  try {
    operation();
    return "none";
  } catch (error) {
    return error.name;
  }
}

console.log(
  caughtName(() => new ArrayBuffer(-1)),
  caughtName(() => new Uint8Array(-1)),
  caughtName(() => new ArrayBuffer(Infinity)),
  caughtName(() => new Uint8Array(1e300)),
);

const indexedBuffer = new ArrayBuffer(4);
console.log(
  caughtName(() => new Uint16Array(indexedBuffer, -1)),
  caughtName(() => new Uint16Array(indexedBuffer, 1)),
  caughtName(() => new Uint16Array(indexedBuffer, 0, 3)),
);
console.log(
  new ArrayBuffer(NaN).byteLength,
  new ArrayBuffer(3.9).byteLength,
  new Uint8Array(NaN).length,
  new Uint8Array(indexedBuffer, 0, undefined).length,
  new Uint8Array([5], 1e300).length,
);

const setBounds = new Uint8Array([1, 2]);
console.log(
  caughtName(() => setBounds.set([9], -1)),
  caughtName(() => setBounds.set([9], Infinity)),
  caughtName(() => setBounds.set([9, 8, 7])),
  setBounds.join(","),
);
setBounds.set([9], 0.9);
console.log(setBounds.join(","));

// Integer-indexed elements are own enumerable properties. Metadata and methods are inherited,
// so every reflective path must draw the boundary in the same place.
const reflected = new Uint8Array([4, 5]);
console.log(Object.keys(reflected).join(","));
console.log(Object.getOwnPropertyNames(reflected).join(","));
console.log(
  Object.hasOwn(reflected, "0"),
  Object.hasOwn(reflected, "2"),
  Object.hasOwn(reflected, "length"),
  Object.prototype.hasOwnProperty.call(reflected, "1"),
  reflected.hasOwnProperty("0"),
  reflected.hasOwnProperty("01"),
);
console.log(
  Object.prototype.propertyIsEnumerable.call(reflected, "0"),
  Object.prototype.propertyIsEnumerable.call(reflected, "2"),
  reflected.propertyIsEnumerable("1"),
);
console.log(
  0 in reflected,
  2 in reflected,
  "01" in reflected,
  reflected["01"],
  "length" in reflected,
  "set" in reflected,
  "push" in reflected,
);
