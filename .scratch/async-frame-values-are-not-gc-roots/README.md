# A value live only in a running async frame is not a GC root

**Status:** open, **on `origin/main` before this branch**, native backend only. Found while merging;
verified against a clean worktree at `2cd616c1` so it cannot be blamed on the merge.

## Reproduction

`repro.js` beside this file, thirteen lines and no libraries:

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

```
node:   got: a,b,c,d,e
native: kiln: native backend refusal: operation=Apply
        detail=an array iterator whose source is not an array or a typed array
```

The interpreter is correct. Removing the inner allocation loop makes it pass; making the loop
synchronous (`repro` with a hand-driven `it.next()` and no `await`) also passes.

## What is actually happening

The refusal names an array iterator, and the iterator is not the problem -- which is what made this
expensive to find. Instrumenting the refusal shows the receiver *is* one of ours and holds three of
its four properties:

```
@@iterated -> tag 0 (undefined)   <- the only one whose value is a heap object
@@index    -> tag 3
next       -> tag 11
@@iterator -> tag 11
```

`kiln_c99_new_iterator` is never called with an undefined source -- checked by instrumenting every
one of its call sites -- so the property was lost after construction. The snapshot array it pointed
at was collected while the iterator was still in use.

## Why it is not rooted

`kiln_c99_gc_unit_roots` marks the realm's statics and nothing else: symbols, native constructor
prototypes, `new_target`, `adopted_this`, `captured_arguments`, and the two frame globals. It does
not mark any executing function's operand stack.

A suspended generator *is* traced -- `KILN_GC_GENERATOR` in `kiln_c99_gc_trace` marks
`frame->locals` and `frame->stack` -- but a generated function runs on a **C-local**
`kiln_value stack[N]`, and that array is only in step with `frame->stack` at a suspension point.
Between them, a value reachable only from the operand stack is invisible to the collector. The
`for await` loop holds its iterator exactly there, across a body that allocates hard.

That is why the synchronous spelling survives and the asynchronous one does not.

## Eliminated, so nobody repeats them

- **Interning.** `KILN_KEYS_CHECK=1` reports no disagreement; `@@iterated` and `@@iterator` are
  distinct interned keys despite sharing a length and a prefix.
- **The inline-slot boundary.** Objects of one to eight properties built by assignment read back
  every property; so do the same keys the iterator uses.
- **Shape tracing.** Marking `inline_slots` in addition to `slots` in `kiln_c99_gc_trace` changes
  nothing.
- **Polymorphic method caches.** One `it.next()` site driven by five receiver kinds is correct.
- **A stale diagnostic.** Tempting, because the detail names something uninvolved -- but tracing
  `kiln_c99_forget_handled_throw` shows the iterator refusal fires and is never recovered. The
  detail is genuine; `Apply` is only the instruction it surfaces through.

## The shape of a fix

Either the operand stack becomes a root for the duration of the call -- a shadow stack the
collector walks, which costs something on every call and is the thing ADR 0050's work is trying to
buy back -- or collection is confined to points where the C-local stack has already been spilled to
`frame->stack`. That is a design call for whoever owns the shape and inline-cache work, which is why
this is a filing and not a patch.

Worth a `conformance/native` probe with the fix. There is none today that allocates hard inside a
`for await`, which is why a corpus of nineteen real bundles found it and the probes did not.
