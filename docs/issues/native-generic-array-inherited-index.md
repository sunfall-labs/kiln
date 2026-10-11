# Native generic `map`/`reduce` miss an index added up the chain mid-walk

Status: closed

`Array.prototype.map` and `reduce`, when a unit calls them through the generic path rather than
the loops lowered at the call site, skip a hole whose index a callback has just added to the
prototype chain. The interpreter and node walk the chain at that index (ECMA-262 §23.1.3.21 step
6.b HasProperty).

```js
const nums = [];
for (let i = 0; i < 50; i++) nums.push(i);
nums.map((x) => x * 2);            // any earlier dense-array call: the unit stops being versioned
const a = [1, , 3];
console.log(JSON.stringify(a.map((x, i) => { if (i === 0) Array.prototype[1] = "late"; return x; })));
// node / interpreter: [1,"late",3]   native: [1,null,3]
const c = [1, , 3];
console.log(c.reduce((acc, x, i) => { if (i === 0) Object.prototype[1] = 10; return acc + x; }, 0));
// node / interpreter: 14             native: 4
```

Found by `conformance/native/array-hof-prepared-fastpath.mjs` while landing the call-site
loop work, which handles this case in its own loops (`inherited_element`). The generic path was
not changed by that work. Candidates: `kiln_c99_array_next_present_index` (methods.c) deciding
from the prototype stores' indexed keys, and the C `map`/`reduce` arms in `dispatch_part_b.c`.

Fixed: `kiln_c99_store_has_indexed_property` (methods.c) looked only at a prototype store's
dictionary list, so an index in its elements (`Object.prototype[1] = 10`) or a shape slot let the
hole be skipped. It now answers yes for any of them; the probe cases are restored.
