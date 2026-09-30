// A call site that has cached a builtin's static method must see an own property written onto
// the builtin afterwards, and see the builtin again once the original is put back.
function keysOf(o) { return Object.keys(o).join(","); }
const out = [];
for (let i = 0; i < 3; i++) out.push(keysOf({ a: i, b: 2 }));
const originalKeys = Object.keys;
Object.keys = function (o) { return ["patched", originalKeys(o).length]; };
for (let i = 0; i < 2; i++) out.push(keysOf({ a: i, b: 2 }));
Object.keys = originalKeys;
for (let i = 0; i < 2; i++) out.push(keysOf({ a: i, b: 2, c: 3 }));
console.log(out.join("|"));

async function main() {
  const seen = [];
  function resolveIt(v) { return Promise.resolve(v); }
  for (let i = 0; i < 3; i++) seen.push(await resolveIt(i));
  const original = Promise.resolve;
  Promise.resolve = function (v) { return original.call(Promise, "patched:" + v); };
  for (let i = 0; i < 2; i++) seen.push(await resolveIt(i));
  Promise.resolve = original;
  for (let i = 0; i < 2; i++) seen.push(await resolveIt(i + 10));
  console.log(seen.join(","));
}
main();

// Bootstrap definitions on callable namespaces must be visible before any site is warm.
console.log(JSON.stringify({ a: 2 }, (_key, value) => typeof value === "number" ? value + 1 : value));
const captureTarget = {};
Error.captureStackTrace(captureTarget);
console.log(typeof captureTarget.stack);

// Reflection must inspect the same callable store that definitions and reads use.
const ownSymbol = Symbol("own builtin key");
Object.defineProperty(Math, "customOwn", { value: 7, configurable: true });
Object.defineProperty(Math, ownSymbol, { value: 8, configurable: true });
console.log(Object.hasOwn(Math, "customOwn"), Object.prototype.hasOwnProperty.call(Math, ownSymbol));
delete Math.customOwn;
delete Math[ownSymbol];
