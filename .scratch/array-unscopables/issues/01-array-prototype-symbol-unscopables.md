# Array.prototype[Symbol.unscopables] Implementation

Status: resolved · Severity: medium

## Goal

Provide a realm-held `Array.prototype[Symbol.unscopables]` object with null prototype and required keys, satisfying ECMA-262 §23.1.3.32.

## Details

1. In `crates/kiln-interpreter/src/realm.rs` (or `intrinsics.rs`):
   - Add `unscopables: RefCell<Option<Value>>` to realm intrinsics.
   - Construct the object with null prototype and properties (`at`, `copyWithin`, etc. all `true`, descriptor `w0e0c1`).
2. Wire property access and own descriptor in `crates/kiln-interpreter/src/intrinsics.rs` so that property reads and `getOwnPropertyDescriptor` return the same cached object.
3. Wire C backend parity in `crates/kiln-backend-c`.
4. Verify ~10 Test262 rows in `built-ins/Array/prototype/Symbol.unscopables`.

## Resolution

- **Interpreter**: Added `unscopables: RefCell<Vec<Option<Value>>>` to `struct Intrinsics` in `crates/kiln-interpreter/src/lib.rs`. Initialized per-realm null-prototype object with the 16 required keys (`at`, `copyWithin`, `entries`, `fill`, `find`, `findIndex`, `findLast`, `findLastIndex`, `flat`, `flatMap`, `includes`, `keys`, `toReversed`, `toSorted`, `toSpliced`, `values`) all set to `true`. Chained in GC roots via `take_roots()`. Routed `table_step`, `tabulated_property`, and `read_property` on `WellKnown::Unscopables` to this intrinsic.
- **Test262**: 10/10 tests in `built-ins/Array/prototype/Symbol.unscopables` pass.
- **C Backend**: Implemented `kiln_c99_get_array_unscopables` in `crates/kiln-backend-c/src/runtime/stores.c` with GC root registration in `boxed.c`, computed property read in `read.c` & `write.c`, descriptor query support in `definitions.c` (`w0e0c1`), own keys support in `keys.c`, and membership testing (`in` operator) in `enumeration.c`.
- **Conformance**: Added `conformance/native/array-unscopables.js` verifying identical output across node, interpreter, and native.
