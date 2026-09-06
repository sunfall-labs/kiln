# `reduce` re-reads the array's length each step; the specification reads it once

Found 2026-09-06 while giving the array intrinsics typed callback loops. `Array.prototype.reduce`
(ECMA-262 §23.1.3.24) takes `len = LengthOfArrayLike(O)` once and iterates `k < len`, so an
element the callback pushes is not visited. Both kiln engines re-read the live length in their
dense-array fast paths, so a callback that grows the receiver runs for the new elements too;
`map` and `filter` capture the length once and agree with node.

```js
const a = [1, 2, 3];
const s = a.reduce((acc, x) => { if (x === 1) a.push(10); return acc + x; }, 0);
console.log(s);   // node: 6   kiln (both engines): 16
```

Triage: bug, language semantics, both engines; the interpreter's fast path and the native
intrinsic loop (generic and typed) should capture `count` before the loop as `map` does.
