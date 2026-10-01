function read(view, key) { return view[key]; }
const views = [new Uint8Array([255, 7]), new Uint8ClampedArray([255, 8]),
  new Int8Array([-1, 7]), new Int16Array([-32768, 8]), new Uint16Array([65535, 8]),
  new Int32Array([-2147483648, 9]), new Uint32Array([4294967295, 10]),
  new Float16Array([1.5, -0]), new Float32Array([1.5, -0]), new Float64Array([NaN, 2.5])];
for (const view of views) {
  console.log(read(view, 0), read(view, 1), read(view, 2) === undefined);
}
const view = new Int32Array([17]);
const order = [];
console.log(read(view, { toString() { order.push('key'); return '0'; } }), order.join(','));
console.log(read(view, '0'), read(view, '01') === undefined, read(view, NaN) === undefined);
console.log(read(view, -0), read(view, 0.5) === undefined);
const big = new BigInt64Array([3n]);
const bigUnsigned = new BigUint64Array([4n]);
console.log(read(big, 0) === 3n, read(bigUnsigned, 0) === 4n);
const moved = view.buffer.transfer();
console.log(read(view, 0) === undefined, new Int32Array(moved)[0]);
