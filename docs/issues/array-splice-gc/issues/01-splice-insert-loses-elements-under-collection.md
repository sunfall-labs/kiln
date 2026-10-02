# Issue 01: `splice` inserting into a small array loses elements when a collection lands mid-insert

Status: resolved
Feature: array-splice-gc
Governing ADR: ADR 0041

Found on 2026-09-07 by the new `mise run native-stress` task, on its first run, in
`conformance/native/inline-array-growth.js`. Unrelated to the JSON walk that task was added for.

## Repro

```js
const a = [2, 4];
a.splice(1, 0, "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8");
console.log(a.length + ":" + a.join(","));
```

```
kiln run <file>                                  10:2,s1,s2,s3,s4,s5,s6,s7,s8,4   (node agrees)
KILN_GC_STRESS=1 kiln run --native <file>        10:2,s1,s6,s7,s8,10,s6,s7,s8,4
```

Ordinary native runs are correct; it needs a collection inside the insert.

## What the wrong answer says about the cause

The length is right (10) and the first and last elements are right, so the shift and the count are
fine. The middle is `s1,s6,s7,s8,10,s6,s7,s8` where it should be `s1,s2,s3,s4,s5,s6,s7,s8`: a run
of inserted values is repeated, and **the new length `10` appears as an element**. A number that
is not in the source text arriving in the elements block means the write went somewhere it should
not have -- the signature of an `elements` pointer captured before an allocation that grew the
array, then written through afterwards, with the insert's own bookkeeping landing in the block.

`splice` growing `[2,4]` by eight has to reallocate, and any allocation is a collection point
under stress.

## Acceptance

- The repro matches the interpreter and node under `KILN_GC_STRESS=1` and `=200`.
- Whatever site is found, check its neighbours in the same file for the same shape: a captured
  `array->elements` (or `object->slots`) held across an allocation. `kiln_c99_json_build_mirror`
  had exactly this shape and was corrected in the same change that added the stress task.
- `mise run native-stress` reports `0 diverged`.

## Resolved 2026-09-07

`splice` and `unshift` filled the gap they had just opened *before* publishing the array's new
`count`, and converting a string argument allocates (`kiln_val_from_boundary` calls
`kiln_c99_val_from_text` for anything longer than one byte) -- so each conversion collected the
string the previous one had stored, because the collector traces an array's first `count`
elements and no further. Both now fill the gap with `KILN_VAL_UNDEFINED`, publish the final
count, and only then convert. The regexp match array in `boxed.c` had the same shape and now
publishes each group as it lands; it was safe only by the coincidence that its 32-entry cache
held the same values.

The invariant is written down at `kiln_c99_array_reserve` (`arrays.c`), which is the function
that creates the untraced room, and pinned by `conformance/native/array-insert-gc.js` -- which
passes without `KILN_GC_STRESS`, so only `mise run native-stress` makes it fail.

