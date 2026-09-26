# 05. Layer check test

Status: ready-for-agent
Blocked by: 01 (ADR 0066)

A workspace test that reads `cargo metadata`, assigns each crate its ADR 0066 layer, and fails on a
normal (non-dev) dependency that points to a higher layer, or across `interp/` and `native/`. The
error names both crates, both layers and ADR 0066.

Until 06 moves crates into layer directories, the layer comes from a table in the test. After 06
it comes from the directory, and the table is deleted.

The current violations are allowed by an explicit list in the test, each naming the issue that
removes it, so the test lands green and the list can only shrink:

- `kiln-module-graph` → `kiln-interpreter`
- `kiln-db` → `kiln-backend-c`
- `kiln-liveness` → `kiln-backend-c`

## Acceptance

- The test runs in the ordinary `mbx test` of the crate that hosts it, so `mise run gate` covers it.
- Adding a new upward edge fails with the message above.
