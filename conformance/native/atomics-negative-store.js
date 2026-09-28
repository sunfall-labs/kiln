const a = new Int32Array(new SharedArrayBuffer(8));
console.log(Atomics.store(a, 0, -1), Atomics.load(a, 0), a[0]);
console.log(Atomics.store(a, 1, -42), Atomics.load(a, 1), a[1]);
console.log(Atomics.add(a, 0, -2), Atomics.load(a, 0));

const immediate = Atomics.waitAsync(a, 0, 0, 10);
console.log(immediate.async, immediate.value);
const expired = Atomics.waitAsync(a, 0, -3, 0);
console.log(expired.async, expired.value);
