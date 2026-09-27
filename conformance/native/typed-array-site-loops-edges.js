// kiln-native: profile=native-release
//
// The edges of the call-site loops over typed arrays (parallel issue 16): a callback that
// detaches the buffer mid-loop reads `undefined` from then on, as the TypeScript's `this[k]`
// does; one that throws stops the loop at that index; a thisArg reaches a strict callback, and a
// callback that writes the view sees its writes; BigInt kinds keep the TypeScript.
const make = () => {
  const t = new Float64Array(6);
  for (let i = 0; i < 6; i++) t[i] = i + 0.25;
  return t;
};
let seen = [];
const view = make();
const mapped = view.map((x, i) => {
  if (i === 2) view.buffer.transfer();
  seen.push(String(x));
  return i;
});
console.log("detach map", Array.from(mapped).join(","), seen.join(","), view.length);
seen = [];
const other = make();
const kept = other.filter((x, i) => {
  if (i === 3) other.buffer.transfer();
  seen.push(String(x));
  return true;
});
console.log("detach filter", Array.from(kept).join(","), seen.join(","));
const third = make();
console.log("detach reduce", third.reduce((acc, x, i) => {
  if (i === 1) third.buffer.transfer();
  return acc + "|" + x;
}, "r"));
try {
  make().map((x, i) => {
    if (i === 4) throw new RangeError("at " + i);
    return x;
  });
} catch (error) {
  console.log("throw", error.name, error.message);
}
const self = { k: 10 };
console.log("thisArg", Array.from(make().map(function (x) { "use strict"; return x + this.k; }, self)).join(","));
console.log("thisArg filter", Array.from(make().filter(function (x) { "use strict"; return x > this.k - 8; }, self)).join(","));
const writes = make();
console.log("writes", Array.from(writes.map((x, i) => { writes[i + 1] = 100; return x; })).join(","));
const big = new BigInt64Array([5n, -3n, 7n]);
console.log("bigint", big.map((x) => x * 2n).join(","), big.filter((x) => x > 0n).join(","), big.reduce((a, x) => a + x, 0n));
