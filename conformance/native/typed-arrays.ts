// One generic native representation must cover every numeric view kind.  The values below make
// wrapping, clamping and float narrowing visible without depending on implementation details.
console.log(new Int8Array([255, 128]).join(","));
console.log(new Uint8Array([256, -1]).join(","));
console.log(new Uint8ClampedArray([-1, 254.5, 255.5, NaN]).join(","));
console.log(new Int16Array([65535, 32768]).join(","));
console.log(new Uint16Array([-1, 65536]).join(","));
console.log(new Int32Array([4294967295, 2147483648]).join(","));
console.log(new Uint32Array([-1, 4294967296]).join(","));
console.log(new Float32Array([1.1]).join(","));
console.log(new Float64Array([1.1]).join(","));

// Views share bytes, subarray keeps sharing them, and the runtime's byte order is explicit.
const buffer = new ArrayBuffer(8);
const words = new Uint16Array(buffer);
const bytes = new Uint8Array(buffer);
words[1] = 0x1234;
console.log(buffer.byteLength, words.length, words.byteLength, words.byteOffset);
console.log(bytes[2], bytes[3]);
console.log(new Uint8Array(buffer, 0, undefined).length, new Uint8Array([5], 1e300).length);

const middle = bytes.subarray(2, 4);
middle[0] = 9;
console.log(words[1], middle.buffer === buffer, middle.byteOffset, middle.length);

const copy = middle.slice();
copy[0] = 42;
console.log(middle[0], copy[0], copy.buffer === buffer);

const filled = new Uint8Array(4);
filled.fill(3, 1, 3);
filled.set([8, 9]);
console.log(filled.join(","));

// Iterators read the live view.  Spread uses the same protocol rather than snapshotting a plain
// Array inside the typed-array implementation.
const iterator = bytes[Symbol.iterator]();
bytes[0] = 7;
console.log(iterator.next().value, [...middle].join("-"));

console.log(
  words.BYTES_PER_ELEMENT,
  Uint16Array.BYTES_PER_ELEMENT,
  words instanceof Uint16Array,
  words instanceof Uint8Array,
  Array.isArray(words),
);
