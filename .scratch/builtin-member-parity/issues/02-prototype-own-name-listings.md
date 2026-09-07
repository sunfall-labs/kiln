# Issue 02: built-in prototype own-name listings, per prototype, on both engines

Status: needs-triage
Feature: builtin-member-parity
Governing ADR: ADR 0043, ADR 0054

Split out of `.scratch/array-prototype-parity/issues/01`, which reported `String.prototype` at 34
names against the interpreter's 50 and resolved without it. Measured against node with
`.scratch/probe-inop/names2.mjs`; `Object.getOwnPropertyNames(P).length`.

## Two different failures were mixed together under one count

**Names an engine published that the language does not have** -- wrong answers, since a read of
them answered a function. Fixed 2026-09-07 in the same change as issue 01:

| lie | engines | cause |
| --- | --- | --- |
| `Map.prototype.add`, `Set.prototype.get`, `Set.prototype.set`, and the same on `WeakMap`/`WeakSet` | native | `kiln_c99_namespace_method` consulted the shared collection method-code range without asking which of the four kinds the prototype was, so every kind answered for every other kind's members |
| `String.prototype.values` | native and interpreter | a string carries `@@iterator` and no `values` beside it; both engines answered `values` with the iterator function |

**Names an engine has not built** -- refusals, not wrong answers, and a listing that included them
would be the same lie facing the other way (`keys.c` says so where it declines to list `length`).
These are the remaining gap and are *not* fixed here: new built-ins must be written in TypeScript
with intrinsics (design.md §5, ADR 0005), and adding one to the prelude C is a review refusal.

## Counts after the 2026-09-07 fix

| prototype | node | interpreter | native | native has not built | interpreter has not built |
| --- | --- | --- | --- | --- | --- |
| `Array` | 40 | 40 | 40 | `length` | `length` |
| `String` | 52 | 49 | 33 | 20, incl. the HTML legacy set (`anchor`, `big`, `blink`, `bold`, `fixed`, `fontcolor`, `fontsize`, `italics`, `link`, `small`, `strike`, `sub`, `sup`), `isWellFormed`, `toWellFormed`, `toLocaleLowerCase`, `toLocaleUpperCase`, `trimLeft`, `trimRight`, `length` | `isWellFormed`, `toWellFormed`, `length` |
| `Object` | 12 | 12 | 7 | `__defineGetter__`, `__defineSetter__`, `__lookupGetter__`, `__lookupSetter__`, `__proto__` | -- |
| `Number` | 7 | 7 | 4 | `toExponential`, `toLocaleString`, `toPrecision` | -- |
| `Function` | 9 | 7 | 5 | `arguments`, `caller`, `length`, `name` | `length`, `name` |
| `Promise` | 4 | 4 | 4 | -- | -- |
| `Map` | 11 | 13 | 10 | `size` | -- |
| `Set` | 17 | 17 | 9 | `size` and the seven set operations (`difference`, `intersection`, `isDisjointFrom`, `isSubsetOf`, `isSupersetOf`, `symmetricDifference`, `union`) | -- |
| `Date` | 47 | 47 | 14 | 33, the whole UTC getter/setter family plus the locale and `to*String` set | -- |
| `RegExp` | 15 | 14 | 4 | 11, the flag accessors (`dotAll`, `flags`, `global`, `hasIndices`, `ignoreCase`, `multiline`, `sticky`, `unicode`, `unicodeSets`) plus `compile` and `source` | `compile` |

`parallelMap` is listed by both engines on `Array.prototype` and by node on none: it is kiln's own
member (ADR 0058), not a gap.

## Two remaining divergences that are not "unbuilt"

* **`String.prototype.toLocaleString` is listed as own on native; node has no own one** (it is
  inherited from `Object.prototype`). The backend reaches one method code,
  `KILN_C99_METHOD_OBJECT_TO_LOCALE_STRING`, for both `Object.prototype`'s and `Array.prototype`'s
  -- and node *does* list an own `toLocaleString` on `Array.prototype`. So the listing walk's rule
  (list a code when the name it carries leads back to it) cannot separate "inherited" from "own"
  by identity the way the interpreter's `intrinsic_owns` does with `tabulated_object_property`.
  Fixing it means giving the two a code each, or teaching the walk which prototype owns which
  shared code. No program is misled about existence -- a read answers a function on both -- only
  about ownership.
* **`Map.prototype.getOrInsert` and `getOrInsertComputed` on the interpreter**, which node does
  not have. They are the Stage 3 upsert proposal; if that is deliberate the row belongs in a
  compatibility note rather than here.

## Acceptance for the unbuilt half

A TypeScript-with-intrinsics implementation per prototype, or a decision per family that the gap
stays and is recorded. `scripts/native-surface-inventory.py --gate` is the existing measurement.
