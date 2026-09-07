# Issue 01: two `Array.prototype` parity gaps found while describing `parallelMap`

Status: needs-triage
Feature: array-prototype-parity
Governing ADR: ADR 0054

Found on 2026-09-07 by `conformance/native/parallel-map-member.ts`, which had to step around both.

## 1. `Array.prototype.map.call(view, fn)` answers a typed view on the interpreter

The spec (and node) answer an ordinary Array: a method borrowed from `Array.prototype` is
generic. The native backend says so in `dispatch.c` and returns an Array; the interpreter returns
a view of the receiver's kind.

```js
const v = new Float64Array([1, 2]);
console.log(Array.isArray(Array.prototype.map.call(v, (x) => x * 2)));
// node, native: true      interpreter: false
```

`parallelMap` inherits whichever answer its engine gives `map`, so the probe compares the
elements and not `Array.isArray`.

## 2. `values` is missing from the native backend's own-name listing of `Array.prototype`

`Object.getOwnPropertyNames(Array.prototype)` lists 40 names on the interpreter and 39 on the
native backend; the symmetric difference is exactly `values`. `[].values` reads fine on native,
so this is the listing (`kiln_c99_builtin_own_keys` in `keys.c`, whose walk asks
`kiln_c99_namespace_method` for each code) not answering `values`' code for the array prototype.
The walk's `< 256` bound was widened to every code in the same change that found this, which
brought `toSorted`, `toReversed`, `toSpliced`, `with` and `parallelMap` into the listing; `values`
is a different miss.
