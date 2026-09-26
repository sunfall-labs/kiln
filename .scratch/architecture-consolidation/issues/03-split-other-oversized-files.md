# 03. Split the other oversized files

Status: ready-for-agent

Same rule as 02 (pure moves, one slice per commit), applied to:

| file | lines (2026-09-26) |
| --- | --- |
| `crates/kiln-native/src/tests.rs` | 12,721 |
| `crates/kiln-native-provider-ffi/src/lib.rs` | 10,034 |
| `crates/kiln-ir/src/compile.rs` | 9,808 |
| `crates/kiln-backend-c/src/tests.rs` | 8,713 |
| `crates/kiln-native/src/lib.rs` | 7,925 |
| `crates/kiln-runtime/src/lib.rs` | 7,384 |
| `crates/kiln-backend-c/src/graph.rs` | 7,211 |
| `crates/kiln-backend-c/src/runtime/dispatch.c` | 7,114 |
| `crates/kiln-interpreter/src/tests.rs` | 7,036 |
| `crates/kiln-module-graph/src/tests.rs` | 6,389 |

Test files split by the feature they test, as `tests/<feature>.rs` modules beside the code.
`dispatch.c` is split only after 14 makes the C prelude order-independent, or it has to be done
in the same order-preserving way as the original `prelude_core.c` split.

## Acceptance

- No Rust source file over 5,000 lines; no C file over 5,000 lines once 14 has landed.
- Per-crate test pass counts unchanged by each slice.
