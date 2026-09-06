# Array.prototype[Symbol.unscopables] Implementation

Status: ready-for-agent · Severity: medium

## Goal

Provide a realm-held `Array.prototype[Symbol.unscopables]` object with null prototype and required keys, satisfying ECMA-262 §23.1.3.32.

## Details

1. In `crates/kiln-interpreter/src/realm.rs` (or `intrinsics.rs`):
   - Add `unscopables: RefCell<Option<Value>>` to realm intrinsics.
   - Construct the object with null prototype and properties (`at`, `copyWithin`, etc. all `true`, descriptor `w0e0c1`).
2. Wire property access and own descriptor in `crates/kiln-interpreter/src/intrinsics.rs` so that property reads and `getOwnPropertyDescriptor` return the same cached object.
3. Wire C backend parity in `crates/kiln-backend-c`.
4. Verify ~10 Test262 rows in `built-ins/Array/prototype/Symbol.unscopables`.
