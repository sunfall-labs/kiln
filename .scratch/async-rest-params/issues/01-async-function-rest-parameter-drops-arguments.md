# Issue 01: an `async function (...rest)` sees no arguments on the native backend

Status: needs-triage
Feature: async-rest-params
Governing ADR: ADR 0028, ADR 0052

## What

An async function declared with a rest parameter receives an empty `rest` on the native backend;
the interpreter and node see the arguments. A plain (non-async) function with the same rest
parameter is fine, and an async function with a positional parameter is fine.

```js
const target = { m(x) { return x + 1; } };
const direct = async function (...a) { return target.m.apply(target, a); };
const noRest = async function (a) { return target.m.apply(target, [a]); };
const sync = function (...a) { return target.m.apply(target, a); };
Promise.all([direct(1), noRest(1), Promise.resolve(sync(1))]).then((v) => console.log(v.join(",")));
// interpreter, node: 2,2,2
// native:            NaN,2,2
```

Found on 2026-09-07 while making `packages/kiln-concurrency` importable on the native backend:
`spawnActor`'s proxy wraps every method in `async function (...fnArgs)`, so every actor call
answers as if called with no arguments (`counter.increment(5)` returns `NaN` where the
interpreter returns 5). The package's own `test.js` runs under node and so never saw it.

## Repro

`.scratch/async-rest-params/repro.mjs` (the snippet above) through `kiln run` and `kiln build`.

## Acceptance

A native probe under `conformance/native/` covering an async function, an async arrow and an
async method each with a rest parameter, called directly, through `apply`, and through a Proxy
`get` trap, matching the interpreter.
