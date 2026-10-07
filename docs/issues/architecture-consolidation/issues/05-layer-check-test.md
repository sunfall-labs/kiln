# 05. Layer check test

Status: resolved

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

## Comments

2026-09-26: Implemented in `crates/tools/kiln-test-selector/tests/layers.rs`, including rule tests
for upward and cross-engine edges. The integration test reads metadata through `mbx`, derives
layers from directories, and reports both crates/layers with ADR 0066 on a violation. No exception
table remains: the workspace has zero forbidden normal dependency edges. Both tests pass in the
ordinary workspace suite. See [verification](../verification.md).
