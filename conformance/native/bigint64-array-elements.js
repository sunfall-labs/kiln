// BigInt64Array and BigUint64Array elements are BigInts, read and written through every path: an
// index, iteration, spread, entries, join, inspection, set, from, and the typed-array methods.
const a = new BigInt64Array([5n, -3n]);
const b = new BigUint64Array([2n ** 64n - 1n, 7n]);
console.log(typeof a[0], a[0], a[1], b[0], b[1]);
let s = 0n; for (let i = 0; i < a.length; i++) s += a[i];
console.log(s, a.at(-1), [...a], Array.from(b), a.includes(5n), a.indexOf(-3n));
console.log(a, b);
console.log(JSON.stringify(Object.keys(a)), a.join(","), String(b));
a[1] = 9n; console.log(a[1], a.map((x) => x * 2n));
const dv = new DataView(a.buffer); console.log(dv.getBigInt64(8, true));
for (const x of b) console.log(x);
console.log(Object.entries(a), a.slice(0, 1)[0], a.subarray(1)[0], new BigInt64Array(a)[1], a.reduce((p, c) => p + c, 0n));
const a3 = new BigInt64Array(3);
a3[0] = -1n; a3[1] = 2n ** 63n; a3[2] = true;
console.log(a3[0], a3[1], a3[2], a3.length);
try { a3[0] = 1; } catch (e) { console.log(e instanceof TypeError); }
const u3 = new BigUint64Array(a3.buffer); console.log(u3[0], u3[1]);
try { new Int8Array(a3); } catch (e) { console.log("mix", e instanceof TypeError); }
try { new BigInt64Array([1]); } catch (e) { console.log("mix2", e instanceof TypeError); }
const c = new BigInt64Array(2); c.set([3n, 4n]); c.set(new BigInt64Array([9n]), 1); console.log(c[0], c[1]);
console.log(BigInt64Array.from([1n, 2n], (x) => x + 1n), a3.filter((x) => x > 0n), a3.sort(), a3.toString(), [...a3.entries()], [...u3.values()][0]);
