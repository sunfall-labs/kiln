# A value live only in a running async frame is not a GC root

**Status:** closed / resolved.

## Reproduction

`repro.js` beside this file:

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

## True Root Cause

The investigation initially deduced that the snapshot array was collected from under the iterator
because `items.tag == 0` (`KILN_VALUE_UNDEFINED`) was read for `"@@iterated"`.

However, the snapshot array was NOT collected; the iterator receiver object still held the array in
`slot[0]` (`slots[0].tag == 10` = `KILN_VALUE_ARRAY`).

What actually happened:
1. `kiln_shape` transition trees persist across GC cycles via `kiln_c99_root_shape`. When dynamically
   interned keys (like `"@@iterated"`) were allocated as GC string blocks, their entries in `heap->keys`
   were not pinned (`entry->pinned == 0`).
2. During GC mark, `KILN_GC_OBJECT` marked `object->slots[index]` but did not mark `object->shape->keys[index]`.
3. Consequently, `kiln_c99_key_purge` identified `"@@iterated"` as an unmarked weak entry and purged it
   from `heap->keys`, allowing the underlying string block to be freed.
4. On subsequent `iterator.next()` calls, `kiln_c99_get(host, receiver, "@@iterated", 10, ...)` called
   `kiln_c99_interned("@@iterated", 10)`. Because the key was purged from `heap->keys`, this returned `NULL`.
5. With `ikey == NULL`, `walk` was 0, causing `kiln_c99_get` to bypass property lookup and answer `undefined`.
6. `KILN_C99_METHOD_ITERATOR_NEXT` observed `items = undefined` and threw:
   `an array iterator whose source is not an array or a typed array`.

## Resolution

1. Added `kiln_c99_pin_interned_key` to pin shape keys in `heap->keys` upon creating a shape transition
   in `kiln_c99_shape_transition`. Pinned keys are marked on every GC epoch by `kiln_c99_key_mark_pinned`
   and are never purged.
2. In `kiln_c99_gc_mark_block` for `KILN_GC_OBJECT`, also mark `object->shape->keys[index]` to ensure
   shape key GC blocks remain marked while referenced by active objects.
3. Added conformance probe `conformance/native/array-iterator-gc.js` testing array iterators across
   GC allocations and spread/apply operations.
