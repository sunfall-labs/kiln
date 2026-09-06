# PRD: Array.prototype[Symbol.unscopables] Realm-Held Intrinsic Object

Status: ready-for-agent
Branch: `feat/array-unscopables`

## Problem Statement

ECMA-262 §23.1.3.32 requires `Array.prototype[Symbol.unscopables]` to be a distinct object with a null prototype, containing 16 properties set to `true`:
`at`, `copyWithin`, `entries`, `fill`, `find`, `findIndex`, `findLast`, `findLastIndex`, `flat`, `flatMap`, `includes`, `keys`, `toReversed`, `toSorted`, `toSpliced`, `values`.
Its property descriptor is non-writable, non-enumerable, configurable (`w0e0c1`).

In Kiln, `Array.prototype[Symbol.unscopables]` answers `undefined`.

## Impact

1. **Language Semantics**:
   `with (arrayLike) { ... }` consults `@@unscopables` so newly added methods do not capture existing variable bindings in lexical scopes.
2. **Conformance**:
   Costs ~10 Test262 rows in `built-ins/Array/prototype/Symbol.unscopables`.

## Architectural Considerations

- `@@unscopables` is an object whose identity is observable across reads:
  `Array.prototype[Symbol.unscopables] === Array.prototype[Symbol.unscopables]` must evaluate to `true`.
- The object must be stored in a per-realm slot on `Intrinsics` (similar to `error_prototype`) rather than constructed dynamically by a pure table function.
- Per ADR 0054, member representation must be consistent between own descriptor queries and property reads.

## Implementation Issues

- **Issue 01**: `issues/01-array-prototype-symbol-unscopables.md` (Implementation and Realm Intrinsic Storage)
