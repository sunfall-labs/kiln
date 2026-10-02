# 20. Writing any built-in makes typed method bodies that use a built-in refuse fatally

Status: resolved

Found during realms issue 04 step 4 (2026-09-27); pre-existing on main (reproduced at 0da8a1c4, one
realm, no concurrency involved).

```js
Math.foo = 1; // any store onto any built-in's own-property store
class Point { constructor(x, y) { this.x = x; this.y = y; }
               dist() { return Math.sqrt(this.x * this.x + this.y * this.y); } }
const pts = []; for (let i = 0; i < 1000; i++) pts.push(new Point(i, i + 1));
let s = 0; for (const p of pts) s += p.dist();
console.log(s);
```

`kiln run --native --profile native-release` stops with `kiln: native backend refusal: ...
operation=CallMethod(dist) detail=CallMethod invocation refused` and exits 1. Node prints the sum.
The interpreter prints the sum. Without the first line both engines agree.

## Analysis

A method called from a typed call site is entered through its typed entry `fn_N_t` (numbers in,
a double out). Inside it, `Math.sqrt(...)` is lowered to a bare `sqrt` guarded by the built-in
speculation `kiln_c99_builtins_unmodified()` (it was `kiln_c99_builtin_store_epoch == 0` before
issue 04 step 4). When the guard fails, the typed entry has nowhere to go: unlike the boxed body
`fn_N_f`, which on a failed speculation `return`s into the generic clone `fn_N`, the typed entry
emits `if (!handled) goto kiln_refusal;`, sets `kiln_c99_typed_status = KILN_C99_CONTROL_ERROR`
and returns the failure double. The call site treats that status as the callee's refusal and
reports it; nothing retries the call through the boxed path.

So the guard is sound -- a realm that patched `Math.sqrt` must not get libm -- but its failure
edge in a typed entry is a hard refusal instead of a deoptimisation. Since issue 04 step 4 the
guard asks the *current realm* (other realms' stores no longer trip it), but any store by the
program itself onto any built-in -- `Math.foo = 1`, a polyfill, `Array.prototype.x = ...` -- still
trips every typed body in the program that speculates on some built-in, because the realm's record
is consulted as a whole (`builtin_stores != 0`) rather than per built-in.

Two independent fixes, both worth doing:

1. **A typed entry must be able to leave.** A failed speculation inside `fn_N_t` should report
   "not taken" (a distinct typed status, like the `KILN_VAL_TYPED_FAIL` path the call site already
   has for other bail-outs) so the call site re-dispatches through the boxed closure call it would
   have made without the typed arm. That turns every such refusal into the slow-but-right answer.
   Check that no typed body has side effects before its first speculation that the re-dispatch
   would repeat (the emitter orders guards first today; verify, or restrict re-dispatch to guards
   that dominate every effect).
2. **Ask about the built-in actually used.** The emitter knows which built-in each speculation
   names (`KILN_C99_BUILTIN_MATH`, `_ARRAY`, `_NUMBER`, `_PROMISE`); the slow half of the guard can
   ask `kiln_c99_realm_builtin_modified(id)` (the per-realm bitmap, which already folds in
   Object/Function/%TypedArray%.prototype) instead of "has this realm stored onto anything".
   `Math.foo = 1` then keeps `Math.sqrt` fast.

## Acceptance

- The probe above prints node's answer natively (add it to `conformance/native/`), also with
  `Math.sqrt = (x) => -1` (which must print what node prints: the patched result).
- `scripts/native.sh` and `scripts/native-realms.sh` unchanged; class-poly instructions within
  noise of before.

## Comments

### 2026-09-27 -- done (branch `work/typed-body-fallback`)

**Cause, confirmed.** The typed entry `fn_N_t` of `dist` inlines `Math.sqrt` behind the built-in
guard, and its failure edge was `goto kiln_refusal`; the call site reports any typed failure as the
callee's refusal. The guard asked "has this realm stored onto any built-in", so `Math.foo = 1` (or
`JSON.foo`, a polyfill) failed it.

**Fix 1 -- a typed entry is not entered on a failed guard.** Rather than a "not taken" status
returned *from inside* the entry (which a callback loop mid-iteration or a typed caller could not
act on without repeating effects), the guard is asked by the call site *before* the entry is
entered, where the boxed call is still available and nothing has run: guards before any effect.
- `CallPlan::typed_guards[proto]` (new): the member guards the entry's refusing speculations name
  (read from the emitted body by a mark, `emit_graph::typed_relies`), unioned transitively over the
  typed entries it calls (`typed_entry_calls`), computed in the existing typed-needs fixpoint.
  Restartable entries rely on nothing (they restart through the boxed body already).
- Every site that enters a typed entry checks it: the method arms (method-path front arm and
  site-cache arm -- a no falls through to the boxed `plain->body` call), direct calls from boxed
  callers (a new `else if (!guard)` branch making the boxed direct call; a result the caller was
  compiled to hold as a double/int is checked, and a non-number/non-integer refuses *by name*
  rather than being reinterpreted), and the typed forms of `map`/`filter`/`reduce`/typed-array
  loops/reduce tree (the typed loop is chosen only if the guard holds; otherwise the generic loop).
- Past that check the in-body guard can fail only if the call itself modified the member. A body
  that reaches the runtime (the only kind that can modify anything) now makes the general call
  from where it is (`kiln_c99_val_call_method_key`, finds the override, no effect repeated) and
  refuses only if the answer is not a number. A body that reaches nothing cannot have modified
  anything since its caller checked, so it does not ask again (a call per entry in a modified
  realm); `KILN_IC_CHECK` builds ask anyway and trap if a call site skipped the check
  (`KILN_C99_TYPED_RELIES_CHECK`). No new runtime needs were added to any typed entry (that would
  have cost `always_inline` and the typed loops' fast forms).

**Fix 2 -- ask about what is relied on, in this realm, after bootstrap.** New per-heap record
`program_builtins` (cleared by `kiln_c99_builtin_bootstrap_done`; `modified_builtins` keeps its
sticky meaning for caches) and `builtin_store_serial`.
- Member-granular `kiln_c99_builtin_member_unmodified(id, "member")` for static-member intrinsics:
  `Math.*`, `Number.is*`, `Array.isArray`, `Promise.resolve` (+ `Promise.prototype.constructor`,
  both the old intrinsic and the await form). It answers from the built-in's own-property store (a
  member is the table's until the store holds its name), remembered per (builtin, member) until the
  realm's next built-in store. So `Math.foo = 1` keeps `Math.sqrt` inlined and typed.
- Object-granular `kiln_c99_builtin_pair_unmodified(A, B)` (Object/Function/%TypedArray%.prototype
  folded in) for array-method fast paths (Array, Array.prototype) and string ones (String,
  String.prototype). Parallel-family, typed-array and kiln-extension sites keep the whole-realm
  `kiln_c99_builtins_unmodified()` (the `_p` clones' bare libm relies on it).
- The cold halves call nothing on their common path (heap from the thread cache inline,
  `kiln_c99_gc_heap_if_cached`); anything that must call is itself `preserve_all`. The first
  version saved all 16 xmm registers per guard: 190 instructions per `dist()` call in a realm that
  wrote `JSON.foo`. `kiln_c99_realm_builtins_unmodified` got the same treatment.

**Probes** (`conformance/native/`, native-release): `typed-entry-builtin-write-unrelated.js` (the
repro verbatim, then `JSON`/`Number`/`Array.prototype` stores; direct, method, map, reduce typed
paths), `typed-entry-builtin-write-used.js` (`Math.sqrt = (x) => -1` first -- prints node's
`-1000`; patch/restore/patch/restore across direct, method, map, reduce, typed-array map; a patch
answering a string; the callee patching `Math.sqrt` mid-call from inside a typed caller and a
typed method), `typed-entry-builtin-write-realms.js` (long pristine stretch, then a patch, for
realms reaching it at different times). All three print exactly node's output (md5-compared), also
with `KILN_IC_CHECK=1`. On 83ee707d the first two refuse (`CallMethod invocation refused`); so did
the mid-call case before the in-body general call.

**Verified** (once, final tree). `mbx test -p kiln-backend-c --offline`: 189 passed, 1 ignored;
emitted_c 36 (ABI surface snapshot re-recorded). New tests
`a_speculation_asks_only_about_the_builtins_it_relies_on` (backend-c: bootstrap stores excluded,
`Math.foo` vs `Math.sqrt`, memo dropped by the next store, Object.prototype folding) and
`typed_entry_call_sites_ask_about_the_members_the_entry_inlines` (kiln-native: the arm asks
`Math.sqrt` only; the entry asks nothing else). `scripts/native.sh`: 265 matched, 1 refused
(shadow-realm, as before), 0 diverged. `scripts/native-realms.sh` N=1,2 on the three probes plus
builtin-intrinsics-see-overrides, builtin-static-method-cache, method-inline-cache,
method-cache-prototype, polymorphic-inline-cache, classes-and-callables, array-methods,
string-methods: 11 matched, 0 diverged; the used/realms probes at N=16: 2 matched.

**Instructions** (`perf stat -e instructions:u`, native-release, class-poly shape with fixed
iterations: 20M `dist()` calls over 1,000 mixed Point/Point3D). Base 83ee707d vs this branch:
pristine 2,006,407,350 -> 2,006,336,178 (equal). Base refuses with any store; this branch:
`JSON.foo = 1` 2,586,386,415 (+29/call: one cold guard call at the site), `Math.foo = 1`
2,866,387,425 (+43/call: the remembered member answer), `Math.sqrt = Math.sqrt` 40,166,306,591
(the member is written, so every call takes the boxed body and the general `Math.sqrt` call --
slow and right).

**Left / risks.**
- The in-body failure edge of a typed entry that reaches nothing is unguarded in ordinary builds;
  it rests on every typed call site asking `typed_guards` (all four kinds do today; `KILN_IC_CHECK`
  traps if one does not). A new kind of typed call site must ask too.
- Refusals that remain, by name: a boxed direct call standing in for a typed entry answering a
  non-number (or non-integer) where the caller was compiled for one, and a typed entry whose own
  call patched a member into answering a non-number. Both need a patched built-in to change its
  answer's type.
- Any post-bootstrap store onto any built-in still makes every guard take its (now cheap) cold half:
  ~30-45 instructions per guarded call. A per-realm hot word would remove that; it needs the
  realm's record reachable from emitted code without a call (a thread-local, i.e. ABI).
- Member granularity covers the static-member intrinsics only; array/string method paths are
  per built-in (Array + Array.prototype), so `Array.prototype.last = f` does take `map` off its
  fast loop.
- The recv-row claim failure at a typed entry's top (`recv_row == NULL`) is still a refusal edge
  (pre-existing, not reached by these probes).
