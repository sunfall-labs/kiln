# 03. Split the other oversized files

Status: resolved

Same rule as 02 (pure moves, one slice per commit), applied to:

| file | lines (2026-09-26) |
| --- | --- |
| `crates/native/kiln-native/src/tests.rs` | 12,721 |
| `crates/native/kiln-native-provider-ffi/src/lib.rs` | 10,034 |
| `crates/compile/kiln-ir/src/compile.rs` | 9,808 |
| `crates/native/kiln-backend-c/src/tests.rs` | 8,713 |
| `crates/native/kiln-native/src/lib.rs` | 7,925 |
| `crates/host/kiln-runtime/src/lib.rs` | 7,384 |
| `crates/native/kiln-backend-c/src/graph.rs` | 7,211 |
| `crates/native/kiln-backend-c/src/runtime/dispatch.c` | 7,114 |
| `crates/interp/kiln-interpreter/src/tests.rs` | 7,036 |
| `crates/compile/kiln-module-graph/src/tests.rs` | 6,389 |

Test files split by the feature they test, as `tests/<feature>.rs` modules beside the code.
`dispatch.c` is split only after 14 makes the C prelude order-independent, or it has to be done
in the same order-preserving way as the original `prelude_core.c` split.

## Acceptance

- No Rust source file over 5,000 lines; no C file over 5,000 lines once 14 has landed.
- Per-crate test pass counts unchanged by each slice.

## Comments

2026-09-26: Split the listed implementation and test files into domain modules. Every workspace
Rust source is now below 5,000 lines. The two C dispatch parts preserve their original order and
concatenate byte-for-byte to the original dispatcher; this does not implement issue 14. Moved
backend-neutral graph tests and DB oracle tests retain their combined counts. The full workspace
suite passes (1,992 passed, nine ignored). See [verification](../verification.md).
