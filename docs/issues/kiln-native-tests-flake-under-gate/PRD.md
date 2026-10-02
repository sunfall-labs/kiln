# PRD: Eliminate ETXTBSY Test Flake Under Full Gate

Status: resolved
Branch: `fix/kiln-native-gate-flake`

## Problem Statement

When `mise run gate` executed under full machine build load, `kiln-native` tests intermittently reported 4-5 failures on `build_graph` or binary launch due to `ETXTBSY` (file busy during compiler execve fork).

## Resolution

In `crates/native/kiln-native/src/worker.rs`:
- Replaced the fixed 12-attempt loop (capping at ~626ms) with a 10-second deadline in `retry_while_executable_is_busy`.
- Added `busy_retry_tests` to ensure windows exceeding 1,500ms cleanly succeed.
