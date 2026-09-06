# `Math.max = f` is not seen by a call site the emitter lowered to an intrinsic

**Status:** open

Found 2026-09-06 while proving the builtin-receiver method cache (`kiln_c99_cached_method_matches`,
`KILN_VALUE_BUILTIN` arm) sees an own property written onto a builtin. It does -- `Object.keys`
and `Promise.resolve` overrides are seen by the next call at every site -- but a `Math` method
call never reaches that cache: `emit_graph/ops/call_method.rs` lowers `Math.max(a, b)` on a
`KILN_VAL_IS_BUILTIN(v) && KILN_VAL_BUILTIN_ID(v) == KILN_C99_BUILTIN_MATH` guard straight to
the C, so the guard passes after the override too.

```js
function maxOf(a, b) { return Math.max(a, b); }
console.log(maxOf(1, 1));
Math.max = (a, b) => a * 100 + b;
console.log(maxOf(1, 1));
```

node prints `1` then `101`; native prints `1` twice.

Triage: bug, native backend, soundness; low frequency (patching `Math` is rare outside test
harnesses and polyfills, and a polyfill assigns before any call). The fix is the same stamp the
method cache uses: the intrinsic guard also compares `kiln_c99_builtin_store_epoch`, which any
write onto a builtin moves, and falls to the generic call when it has moved.

Also seen: `delete Object.keys` leaves native answering from the builtin table where node then
throws `TypeError: Object.keys is not a function`. A deletion of a builtin's own member is not
modelled (the store holds overrides, not absences).
