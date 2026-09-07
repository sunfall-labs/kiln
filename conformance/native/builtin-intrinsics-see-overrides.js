// A builtin intrinsic the emitter inlines (Math.max, Math.sqrt, Promise.resolve) must see an
// own property written onto the builtin afterwards, and the original when it is put back.
function maxOf(a, b) { return Math.max(a, b); }
function sq(x) { return Math.sqrt(x); }
const out = [];
for (let i = 0; i < 3; i++) out.push(maxOf(i, 1) + ":" + sq(i * i));
Math.max = (a, b) => a * 100 + b;
for (let i = 0; i < 2; i++) out.push(maxOf(i, 1) + ":" + sq(16));
Math.sqrt = (x) => -x;
out.push(sq(9));
async function res() {
  const seen = [];
  for (let i = 0; i < 2; i++) seen.push(await Promise.resolve(i));
  const original = Promise.resolve;
  Promise.resolve = function (v) { return original.call(Promise, "p:" + v); };
  for (let i = 0; i < 2; i++) seen.push(await Promise.resolve(i));
  Promise.resolve = original;
  seen.push(await Promise.resolve(Promise.resolve("inner")));
  console.log(out.join(" "), seen.join(","));
}
res();
