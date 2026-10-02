// kiln-native: profile=native-release
//
// An element store into a typed array converts the value as the view asks: `ToBigInt` for a
// BigInt view, `ToNumber` for the rest -- and `ToNumber` of an object runs its `valueOf`, which
// can throw. The release fast path converted anything that was not a number with the pure
// runtime conversion instead: a BigInt stored `0n` (`BigInt64Array.prototype.set` from an array
// lost every element), and an object stored NaN without calling `valueOf` or letting it throw.
// The generic element write had the same pure conversion (docs/issues/parallel-builtins issue 18).
function setAll(t, src, offset) {
  const off = offset === undefined ? 0 : offset;
  for (let i = 0; i < src.length; i++) t[off + i] = src[i];
  return Array.from(t, String).join(",");
}
console.log(setAll(new BigInt64Array(2), [3n, -4n]), setAll(new Float64Array(2), [1.5, 2.5]));
const c = new BigInt64Array(2); c.set([3n, 4n]); c.set(new BigInt64Array([9n]), 1);
console.log(c[0], c[1]);

let calls = 0;
const seven = { valueOf() { calls++; return 7; } };
console.log(setAll(new Float64Array(1), [seven]), setAll(new Int8Array(2), ["12", undefined]), calls);
const f = new Float64Array(1);
f[0] = seven;
console.log(f[0], calls);
try {
  setAll(new Float64Array(1), [{ valueOf() { throw new Error("boom"); } }]);
  console.log("no throw");
} catch (error) {
  console.log("threw", error.message);
}
try {
  setAll(new BigInt64Array(1), [1]);
  console.log("no throw");
} catch (error) {
  console.log("threw", error instanceof TypeError);
}
