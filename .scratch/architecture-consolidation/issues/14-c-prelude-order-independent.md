# 14. Make the C prelude order-independent

Status: ready-for-agent

`crates/kiln-backend-c/src/runtime/README.md`: "Order is load-bearing." The domain files are
concatenated by `runtime_parts!` and `core_parts!` in `prelude.rs`, and a definition used before
its declaration breaks the build depending on file order.

Generate one prototypes header from the definitions in `runtime/*.c`, checked in and verified by a
test, and include it first. After that, file order does not matter, a new file needs no place in a
list, and each file can in principle be a separate translation unit with its own cache entry.

## Acceptance

- Reordering the lists in `prelude.rs` does not change whether the prelude compiles.
- The "Order is load-bearing" section of `runtime/README.md` is deleted.
- `scripts/native.sh` passes; the native cache keys change once, as expected for a prelude edit.
