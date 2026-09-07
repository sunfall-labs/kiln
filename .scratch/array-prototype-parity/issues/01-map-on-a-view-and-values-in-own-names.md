# Issue 01: two `Array.prototype` parity gaps found while describing `parallelMap`

Status: resolved
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

## Resolution (2026-09-07)

Both halves fixed. `conformance/differential/array-generic-on-view.js` and
`conformance/native/array-prototype-own-names.ts` pin them.

### 1 was two bugs facing opposite ways, one cause

`typed_result` turned a walk's plain-array result into a view whenever the *receiver* was a view,
without asking which of the two tables the method came from. That is the wrong question: a
`%TypedArray%.prototype` method builds with `TypedArraySpeciesCreate` and owes a view, an
`Array.prototype` method builds with `ArraySpeciesCreate` and owes a plain array for any receiver
that is not an Array. The interpreter already carries the answer in `typed_array_method`, the flag
`array_like_length` consults for the same reason, so the conversion now asks it too.

The flag was set only in `invoke_dispatch`, the path a property read takes. `call`, `apply`,
`bind` and `Reflect.apply` arrive through `call_value_inner`, which re-dispatched a
`TypedArrayGeneric` to its `Array.prototype` twin without setting it -- so gating on the flag
alone would have broken `%TypedArray%.prototype.map.call(view, f)`, which was right before only
because the receiver's kind decided everything. `call_value_inner` now sets the flag exactly as
`invoke_dispatch` does.

That path was also missing `typed_result` entirely, so it dropped the receiver's kind on the
floor: `%TypedArray%.prototype.slice.call(view)` answered an ordinary array, and
`Array.prototype.reverse.call(view)` rearranged a snapshot instead of the view. It now ends the
same way `invoke_dispatch` does.

Measured against node, `Array.prototype` borrowed onto a `Float64Array`: `map` and `filter` were
wrong (view, should be Array); `slice`, `concat`, `flat`, `flatMap`, `toSorted`, `toReversed`,
`toSpliced` and `with` were already right. Through `%TypedArray%.prototype` with `call`/`bind`/
`Reflect.apply`: `map`, `filter` and `slice` were wrong (Array, should be view). All now match.

### 2 was a name that never led back to its own code

`KILN_C99_METHOD_ARRAY_VALUES` is numbered in the generic method group, not the array-lookup one,
so `kiln_c99_array_method("values")` never answered and `kiln_c99_namespace_method` had no case
for it. `Array.prototype.values` was therefore a refusal where `[].values` is a function -- an
array *instance* has had an explicit case beside `@@iterator` in `read.c` since the iteration
protocol was written down, and the prototype object had none. The own-keys walk lists a code only
when the name it carries leads back to it, so the listing skipped `values` too. `namespace_method`
now answers it for `Array.prototype`, beside the `@@iterator` case it already had.
`Object.getOwnPropertyNames(Array.prototype)` is 40 names on both engines and the sorted lists are
identical. `keys` and `entries` were already listed, and `[].values === [][Symbol.iterator]` holds
on both.

## Found alongside, not fixed

* `Array.prototype.concat.call(view, 9)` spreads the view; node treats it as one element, since a
  view is not concat-spreadable. Interpreter only.
* `Array.prototype.splice.call(view, 1, 2)` answers an empty array; node throws a `TypeError`
  because a view's length cannot change. Interpreter only.
* `Array.prototype.sort.call(view)` and `.fill.call(view)` answer a fresh array and leave the view
  untouched; node rearranges the view and returns it. `reverse` was in the same family and is
  fixed, because `rearranges_in_place()` names it and not the other two -- extending that
  classification is a judgement about `sort`'s `SortIndexedProperties` path, not a rename.
* `"map" in Array.prototype` is `false` on the native backend for every member, where
  `hasOwnProperty` is `true`; on the interpreter it is `true` for every name, including one no
  engine has (`"nope" in Array.prototype`). Node is `true` for members and `false` otherwise. Both
  are wider than this issue; the native probe says why it does not ask `in`.
* `Object.getOwnPropertyNames(String.prototype)` is 34 names on the native backend against 50 on
  the interpreter, the same shape of gap this issue's half 2 was for `Array.prototype`.
