# Shape Key Purging During GC Corrupts Iterator Next Lookups

Status: resolved · Severity: high

## Comments

Resolved 2026-09-04:
- Pinned shape keys in `heap->keys` during transition via `kiln_c99_pin_interned_key`.
- Marked `object->shape->keys` during GC object marking.
- Added regression test `conformance/native/array-iterator-gc.js`.

## Repro

See `repro.js` in this directory:
```js
const tick = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms));
async function main() {
  const got = [];
  for await (const v of [tick(1, "a"), tick(1, "b"), tick(1, "c"), "d", "e"]) {
    got.push(v);
    for (let i = 0; i < 60000; i++) { const junk = { x: i, y: [i], z: { i } }; if (junk.x < 0) got.push(junk); }
  }
  console.log("got:", got.join(","));
}
main();
```
