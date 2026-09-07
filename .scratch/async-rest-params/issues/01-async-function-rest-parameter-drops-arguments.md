# Issue 01: an `async function (...rest)` sees no arguments on the native backend

Status: resolved
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

## Resolved 2026-09-07

`conformance/native/async-rest-params.ts` covers it, matching the interpreter line for line.

The rest array was built after the split, from an argument list the driver did not carry. An
async function is the one suspending shape whose parameter binding is *not* run at the call:
`binds_at_call` in `crates/kiln-backend-c/src/lib.rs` excludes it deliberately, so that an
instantiation failure rejects the promise rather than throwing out of the call, which leaves its
prologue -- `RestArguments` among it -- to run inside the first `kiln_c99_async_step`. That step
entered the body with `NULL, 0` for the argument list, so `RestArguments`' loop
`for (extra = from; extra < argc; ++extra)` ran zero times. Generators and async generators were
right because `kiln_c99_bind_parameters` runs their prologue at the call with the real list.

`kiln_c99_async_step` now takes the call's arguments, passed only by the entry in
`kiln_c99_call` that runs the prologue -- the caller's list is still alive there, since the first
step is synchronous -- and `NULL, 0` by every resumption, which is behind the prologue by then.

Wider than filed: `arguments` was empty in an async function for the same reason
(`async function f(a) { return arguments.length }` answered 0 where node and the interpreter
answer 1), and is fixed by the same change.
