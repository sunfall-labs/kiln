// kiln-native: profile=native-release
//
// An array and a typed array reach the same element read, element write and `length` read.
// The release profile's sites remembered the first array they saw and took every later
// buffer-tagged value for one: a view with a byte offset then read its buffer pointer as the
// elements and its offset as the count, so `length` answered 8, a read answered a denormal, and
// a write landed in the view's buffer header instead of its bytes. The kind is asked each time.
// Element reads of every Number kind take the inline load (`kiln_typed_load`); a BigInt kind is
// left to the generic read.
function at(a, i) { return a[i]; }
function put(a, i, v) { a[i] = v; }
function len(a) { return a.length; }

const array = [10, 20, 30];
put(array, 1, 21);
console.log(at(array, 1), len(array));

const buffer = new ArrayBuffer(64);
const bytes = new Uint8Array(buffer, 8, 4);
put(bytes, 1, 7);
console.log(at(bytes, 1), len(bytes), new Uint8Array(buffer)[9], buffer.byteLength);

const views = [
  new Int8Array([-3]), new Uint8Array([250]), new Uint8ClampedArray([300]), new Int16Array([-300]),
  new Uint16Array([65000]), new Int32Array([-70000]), new Uint32Array([4294967295]),
  new Float32Array([1.5]), new Float64Array([-2.25]),
];
console.log(views.map((view) => at(view, 0)).join(" "));
let sum = 0;
for (let i = 0; i < 16; i += 1) sum += at(new Float64Array(buffer, 16, 4), i & 3);
console.log(sum, at(bytes, 9));
