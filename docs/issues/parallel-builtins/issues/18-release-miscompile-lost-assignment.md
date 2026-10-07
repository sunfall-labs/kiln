# 18. Release miscompile: a loop's assignment is lost after a string append

Status: resolved

Found by issue 17. In specialised (release, non-generic) code the native backend drops `from = end`
in this loop, so `from` stays `0`, `indexOf(needle, from)` finds the same match forever and the
program hangs at 100% CPU. It is `web.ts`'s `headerListRemove` (`Headers.set`/`delete`), hidden
only because the bootstrap is emitted generic-only; the same shape in any small release *user*
module is wrong today.

```js
function headerListRemove(list, needle) {
  let at = list.indexOf(needle);
  if (at < 0) return list;
  let kept = "";
  let from = 0;
  while (at >= 0) {
    kept += list.slice(from, at);      // without this line it is correct
    const end = list.indexOf("\n", at + needle.length);
    if (end < 0) return kept;
    from = end;                         // lost
    at = list.indexOf(needle, from);
  }
  return kept + list.slice(from);
}
```

Blocks issue 17's default (`KILN_BOOTSTRAP_SPECIALIZED`).

## Acceptance

- The smallest program with the shape is a `conformance/native` probe built at `native-release`
  and matches the interpreter.
- The root cause is fixed where it is, with a `kiln-backend-c` unit test pinning the pattern.
- The rest of the optimisation that dropped the store is checked for the same flaw.
- Every `conformance/native` probe built at `native-release` (with a timeout) matches or is filed.
- `scripts/native.sh` 0 diverged.

## Comments

2026-09-27, branch `work/release-miscompile`.

**Root cause: the assignment was never lost; the argument was.** The smallest program with the
fault has no loop and no store:

```js
function f(list, needle) {
  let kept = "";
  kept += list.slice(0, 1);                          // a string: the fast clone exits here
  return kept + list.indexOf("x", 1 + needle.length); // interpreter a3, native-release a1
}
f("axbxcx", "ab");
```

A release body is *versioned*: a fast clone that speculates (here: `slice`'s answer feeds `+=`,
so it is guessed a number; `needle.length` feeds `+`, so it is guessed a number) and a generic
clone the fast one exits into mid-body. The clones share every SSA variable. `emit_blocks`
computed the live unboxed twins once, against the fast clone's proof -- which includes the
guesses -- and handed the same sets to both clones. In the generic clone `1 + needle.length` is
not proven (a string `length` would make it a string), so its emitter writes only the box; but
the `indexOf` argument read `d42`, the twin, because it was live. After the exit at `slice` the
twin was the fast clone's stale `0.0`, so `indexOf("\n", 0)` found the list's first `\n`, `end`
was always 0, `from = end` stored 0, and `at = indexOf(needle, 0)` never moved: the loop in
`headerListRemove` ran forever. (`from` *was* assigned -- the value was wrong.)

**Fix (`529c7c86`, `emit_graph/body.rs`).** `split_clone_twins`: the generic clone reads a twin only
for a value its own (baseline) proof covers and reads everything else boxed; a baseline number the
fast clone carries only as an int gets a double twin in both. The fast clone may have defined such a
value and dropped its box, so `route_exits_through_boxing` sends each exit into the generic clone
through a stub that boxes, from their twins, exactly the values defined before that exit
(dominance) and live after it (a backward liveness over the fast graph). Boxing them on every
definition instead cost `bench/non-io/class-poly.mjs` ~7% on the hot path, and so did boxing every
kept value at every exit; the precise form is at parity (class-poly 1.69 vs 1.69 ms best,
object-literal within ~2%, the other cpu/non-io benches unchanged).

**The class.** This is a property of versioning, not of `+=`: any speculated value (a property read
feeding arithmetic, a method call feeding arithmetic, a guessed parameter) consumed after an exit
by an operation that reads twins without a test -- a call argument, a typed store, a phi move, a
branch on a comparison -- read a value the generic clone never wrote. `ts-annotations` diverged at
`native-release` on main for the same reason and matches now. The restartable typed clones (`_t`)
restart from the top and share nothing, so they do not have it.

**Tests.** `executable_generic_clone_reads_a_speculated_sum_boxed_after_an_exit` (answered 12 --
`indexOf` from 0 -- without the fix) and
`executable_fast_clone_exit_boxes_a_speculated_value_it_never_boxed` (a pre-exit product the fast
clone never boxed; fails at run time without the exit boxing), both in `tests/runtime.rs`;
`conformance/native/release-speculated-twin-after-exit.js` (header `profile=native-release`) runs
the shapes, `headerListRemove` with a guard, a failing speculation, and a loop-carried sum.

**The `native-release` corpus pass.** Every `conformance/native` probe was copied with its profile
forced to `native-release` (a `kiln` wrapper with `timeout -k 10 900`, and a copy of `native.sh`
that keeps its results and does not stop at the first broken build -- the real one exits there and
deletes the rest). Before these commits, with the specialized bootstrap: 252 matched, 15 diverged,
1 broken, 4 refused, 1 failed. The pass found, and this branch fixes:

- `9072a86c` a receiver with no shape (a `Date`) matched an unclaimed receiver row and read
  `slots[UINT32_MAX]`: every `d < e`, `${d}`, `d == n` crashed with the specialized bootstrap
  (`Date.prototype[Symbol.toPrimitive]` reads `this.valueOf`), and a two-line user module crashed
  on main. 10 probes.
- `d10e7996` typed-array stores: a BigInt stored `0n` (`BigInt64Array.prototype.set` from an array),
  and an object stored NaN without calling `valueOf` -- in the release fast path and in the generic
  element write (every profile).
- `644226e8`, `539bef64` `Math.clz32`/`Math.imul` converted an object without `valueOf` and cast
  NaN/Infinity to `int64_t`.
- `c1d557ee` `yield yield* inner()` did not compile in any profile (a delegate completion and the
  next `yield` shared a resume id); the release source DCE writes that shape, so
  `generator-protocol` stopped building at release.

After them, the pass answers the same three ways -- specialized bootstrap, generic bootstrap
(`KILN_BOOTSTRAP_SPECIALIZED=0`), and specialized with every speculation forced to exit
(`KILN_SPECULATE=exit`): **265 matched, 5 diverged, 4 refused of 273** (plus `generator-protocol`
and `kiln-concurrency-engine-method` matching in-tree: the first is fixed after that pass's binary,
the second imports `../../packages/` and cannot run from a copy). All the rest are pre-existing,
release-only, in user modules, and the same with the compiler before this issue:

- issue 19: the release source DCE (Oxc) drops coercions and throwing calls --
  `coercion-effects`, `async-generator-coercion-loop`, `date-value-argument`, `text-escaping`;
- issue 20: an integer remainder loses `-0` through its int twin -- `numeric-remainder-boxing`;
- issue 21: a typed method call refuses a receiver its typed clone cannot take --
  `speculative-loop`, `prototypes`, `method-cache-own-property` (refusals, not divergences);
- `shadow-realm` refuses in every profile, as before.

Acceptance: the repro is a probe at `native-release` and matches; the cause is fixed with unit
tests; the class is covered (the exit stress pass included); every probe at `native-release`
matches or is filed. Test results are recorded on issue 17.
