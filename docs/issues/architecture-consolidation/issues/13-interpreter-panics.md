# 13. Remove panics that user code can reach in the interpreter

Status: ready-for-agent
Blocked by: 02 (easier after the split, not strictly required)

`kiln-interpreter/src/lib.rs` has about 78 `unwrap`/`expect` and 101 `panic!`/`unreachable!`
(grep, 2026-09-26). A panic caused by a user program is a crash with no JavaScript stack.

For each: either it is an invariant the compiler guarantees (keep it; the message names the
invariant), or user input can reach it (turn it into a thrown JavaScript error or a
source-attributed refusal, and add a `conformance/regress` case that reaches it).

Work family by family after 02, not as one sweep.

## Acceptance

- Every remaining `panic!`/`unreachable!`/`expect` message names the invariant it relies on.
- Each converted site has a regression test.
