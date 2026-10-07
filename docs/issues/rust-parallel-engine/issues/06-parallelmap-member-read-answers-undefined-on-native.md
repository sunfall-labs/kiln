# Issue 06: `typeof [].parallelMap` is `undefined` on the native backend and `function` on the interpreter

Status: resolved
Feature: rust-parallel-engine
Governing ADR: ADR 0054, ADR 0058

## What

`parallelMap` is lowered by name at a `CallMethod` site on both engines (the interpreter's
`CallbackMethod::ParallelMap`, the native backend's sequential and pooled lowerings), but the
native backend does not describe it as a member of `Array.prototype`, so a *read* of the member
answers `undefined` there:

```js
console.log(typeof [].parallelMap, typeof Array.prototype.parallelMap);
// interpreter: function function
// native:      undefined undefined
```

This is the class CLAUDE.md calls the expensive answer: nothing stops, and a program that
feature-tests before calling takes the wrong branch. `packages/kiln-concurrency/index.js` does
exactly that and then hits a refusal on `Object.defineProperty(Array.prototype, ...)`, so the
package cannot be imported by a native program at all; the two benchmarks under `bench/non-io`
key their polyfill on the host's identity to stay clear of it.

## Fix

Describe `parallelMap` once, the way ADR 0054 describes every built-in member, so a read answers
a builtin callable on the native backend as it does on the interpreter; then the `typeof` guard
in the package works and the benchmarks can drop the identity check.

## Repro

`.scratch` probe: the five-line file above through `kiln run` and through `kiln build`.

## Resolution (2026-09-07)

Described once on each engine. Native: one method code on the append-only tail
(`KILN_C99_METHOD_ARRAY_PARALLEL_MAP`, `ArrayLookup`), its name in `methods.c`, its row in
`kiln_c99_is_array_member`, and dispatch identical to the generic `map` so a call through a read
value is `map` under its own name on an array, a typed view or an array-like. Interpreter:
`parallelMap` added to `ARRAY_MEMBERS`, which is what `getOwnPropertyNames` lists. The own-keys
walk in `keys.c` stopped at code 256, so every tail-coded member (`toSorted`, `toReversed`,
`toSpliced`, `with`, and this one) was missing from native's listing and `hasOwnProperty` refused
it; it now walks to `kiln_c99_method_code_end()`.

Probe: `conformance/native/parallel-map-member.ts` (typeof, identity, descriptor, name,
hasOwnProperty, getOwnPropertyNames, three indirect-call shapes, a view and an array-like receiver,
holes, the receiver argument). `packages/kiln-concurrency` and both benchmarks now guard on
`typeof Array.prototype.parallelMap`, and the package imports on the native backend.
Two parity gaps found alongside are filed as `docs/issues/array-prototype-parity/issues/01`, and the
actor path's `NaN` as `docs/issues/async-rest-params/issues/01`.
