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

function caughtName(operation: () => unknown) {
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
console.log(
  caughtName(() => new Uint16Array(buffer, -1)),
  caughtName(() => new Uint16Array(buffer, 1)),
  caughtName(() => new Uint16Array(buffer, 0, 5)),
);
console.log(
  new ArrayBuffer(NaN).byteLength,
  new ArrayBuffer(3.9).byteLength,
  new Uint8Array(NaN).length,
  new Uint8Array(buffer, 0, undefined).length,
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

// Each concrete constructor owns one prototype marker; those markers share the common typed-array
// prototype that carries the methods and toStringTag accessor.
console.log(
  reflected[Symbol.toStringTag],
  Object.prototype.toString.call(reflected),
);
console.log(
  typeof Uint8Array.prototype,
  Uint8Array.prototype.constructor === Uint8Array,
  Uint8Array.prototype.BYTES_PER_ELEMENT,
);
console.log(
  typeof Uint8Array.prototype.set,
  typeof Uint8Array.prototype.subarray,
  typeof Uint8Array.prototype.values,
);
console.log(
  Object.hasOwn(Uint8Array.prototype, "constructor"),
  Uint8Array.prototype.hasOwnProperty("BYTES_PER_ELEMENT"),
  Object.hasOwn(Uint8Array.prototype, "set"),
);
console.log(
  Object.getPrototypeOf(reflected) === Uint8Array.prototype,
  Object.getPrototypeOf(Uint8Array.prototype) ===
    Object.getPrototypeOf(Int8Array.prototype),
);
