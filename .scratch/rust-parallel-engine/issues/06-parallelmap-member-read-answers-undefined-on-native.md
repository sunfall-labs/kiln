# Issue 06: `typeof [].parallelMap` is `undefined` on the native backend and `function` on the interpreter

Status: needs-triage
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
