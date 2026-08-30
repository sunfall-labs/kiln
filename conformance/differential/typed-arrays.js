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
console.log(Array.from(list).join(","), [...list].join("-"));
for (const value of new Uint8Array([7, 8])) console.log("v", value);

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
