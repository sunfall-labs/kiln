# 21. A typed method call refuses a receiver its typed clone cannot take

Status: resolved

Found by issue 18's `native-release` pass. A method whose typed clone returns a number (`val()
{ return this.v; }`, read through a double receiver row) is called through that clone from a
typed caller. When the receiver's shape has no double row -- the same class holding a string in
`v` -- the caller's typed arm still calls `ku_fn_N_t(depth, receiver, NULL)`; the clone cannot
claim a row, returns `KILN_C99_CONTROL_ERROR`, and the caller propagates it as a refusal instead
of calling the boxed body (`plain->body`) as the generic arm below it would have.

```js
class V { constructor(v) { this.v = v; } val() { return this.v; } }
const objs = [new V(1), new V("x"), new V(3)];
function sumVals() { let s = 0; for (let i = 0; i < objs.length; i++) s += objs[i].val(); return s; }
console.log(sumVals());
// interpreter and native-dev: 1x3
// native-release: native backend refusal: instruction=16 operation=CallMethod(val)
//                 detail=CallMethod invocation refused receiver_tag=9
```

Refused at `native-release` only, and the same with the compiler before issue 18:
`speculative-loop`, `prototypes`, `method-cache-own-property` (the last two refuse at the same
kind of site, `CallMethod(read)` / `CallMethod(next)`; confirm they are this and not a sibling).

## Acceptance

- A typed method arm whose receiver has no typed row (or whose typed clone declines at entry)
  takes the boxed call, with no effect run twice; a refusal from deeper inside a typed clone
  stays a refusal.
- The three probes match at `native-release`; `scripts/native.sh` 0 diverged.

## Comments

2026-09-27, branch `work/release-bugs`.

**Root cause** as filed. A typed method arm entered `ku_fn_N_t(depth, receiver, NULL)` when the
receiver's shape had no row in the entry's typed table, and the entry tried to claim one at its
door. A shape whose `this` fields are not the representation the entry reads (a double mask:
`new V("x")`) cannot be claimed. So the entry failed with `KILN_C99_CONTROL_ERROR` before doing
anything, and the caller propagated that as a refusal instead of taking the boxed call.

**Fix** (`backend-c:` commit). This follows the call-site-guard approach of realms issue 20: check
before any effect. Each typed entry that reads a row now has a sibling
`{sym}_fn_N_t_row(obj)`: the same find-or-claim its prologue does, against the same table with the
same double mask (`emit_graph/sites.rs::typed_row_claimer`, emitted beside the entry and declared
in `lib.rs` where the entry reads the row). The site-cache typed arm in
`emit_graph/ops/call_method.rs` now reads
`if (matched && (__row = ..._t_row(obj)) != NULL) { enter the entry with the row }`. A NULL
falls through to the arm's boxed `plain->body` call, before the callee has run, so nothing runs
twice. The method-path front arm already only matched learned shapes with rows. A failure from
*inside* the entry (a throw, a refusal deeper in) is still propagated exactly as before. The
entry's own door check stays, for the direct-call and callback paths, which pass `NULL`.
`parallel.rs` and the parallel lowering are untouched.

**Tests.** `kiln-native`: `release_typed_method_call_takes_the_boxed_call_for_a_receiver_without_a_row`
(the repro, `1x3`). Probe `conformance/native/release-typed-method-receiver-without-row.js`
(`profile=native-release`) covers the repro, an argument-taking method (`twice(k)` over a string
`v` gives NaN), a strict `this` read through a method value, `map` callbacks over mixed receivers,
and a throw from inside the typed method staying a throw. On base it refuses at
`CallMethod(val) receiver_tag=9`; on the branch it matches.
`speculative-loop`, `prototypes` and `method-cache-own-property` match at `native-release`. All
three were this bug (they needed no other change).

**The release corpus pass** (every `conformance/native` probe forced to `native-release`, a copy
of `native.sh` with `timeout -k 10 900` per native run and no stop at the first broken build, on
the branch's final compiler): **279 matched, 0 diverged, 1 refused** -- `shadow-realm`, which
refuses in every profile. `scripts/native.sh`: 279 matched, 1 refused (`shadow-realm`), 0 diverged.
