# Structural refactor verification — 2026-09-26

Scope: issues 02, 03, 05 and the implementation of 06 / ADR 0066. This is not completion of the
whole architecture-consolidation PRD. No new built-ins or CI changes are included.

Final status: the follow-up fixed the allocation abort and the full gate is green. The initial
failure evidence below is retained; see [quality and resource review](quality-and-resource-review.md)
for the fix, final gate evidence and timing limitations.

The subsequent merge of newer upstream work is reviewed separately in
[integration review](integration-review.md); the measurements below describe the original input.

## Trees and tools

- Input commit: `30c1d54831c1200c59dec20c16ee2b18d8f2595f`.
- Implementation worktree: `~/Developer/kiln-architecture-consolidation`.
- Clean comparison worktree: `~/Developer/kiln-architecture-baseline`.
- Cargo builds, tests, metadata and dependency queries use mise's `mbx`; formatting uses
  `cargo fmt`. Validation was completed before committing the change.
- Native validation uses Ubuntu Clang 18 extracted under `~/.cache/kiln/clang-18`, with
  `CC=$HOME/.cache/kiln/clang-18/usr/bin/clang-18` and
  `LD_LIBRARY_PATH=$HOME/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:$HOME/.cache/kiln/clang-18/usr/lib/llvm-18/lib`.
  The host GCC fails native compilation on the unchanged input too; Clang's target is
  `x86_64-pc-linux-gnu`. Gate concurrency uses `KILN_JOBS=4`.

## Structural checks

- All 37 workspace packages belong to a layer directory. The metadata test finds zero upward
  or cross-engine normal dependencies, without an allowlist.
- `mbx tree -p kiln-backend-c -e normal` contains no interpreter or module-evaluation crate.
- Building `kiln-native`, touching interpreter source, then building again recompiles no native
  crate (the second invocation finishes in 0.07 seconds). This is a dependency smoke check, not a
  benchmark of semantic edits or cold compilation; content-addressed caching can answer touches.
- Every workspace Rust source is below 5,000 lines; the largest is frontend `lib.rs` at 4,987.
  Interpreter `lib.rs` drops from 49,767 to 2,451 lines after responsibility-based regrouping.
- Function-body audits retain all 1,546 interpreter functions, all 308 IR functions, all 487
  native functions, all 353 provider functions and all 785 runtime functions. Necessary API,
  path, visibility, test-source-scan and cache-fingerprint changes are reviewed separately.
- The C dispatcher parts concatenate to the original bytes, SHA-256
  `1691a5c6985a1b4cc4e9e365482b8dfbb720957c33204a868e8815d49d97e26e`.
- The provider inventory's 74 source paths exist after the move. Native provider packaging and
  linking are exercised by native tests, not just workspace compilation.
- `cargo fmt --all -- --check` and `git diff --check HEAD` pass.

## Test evidence

Logs live under `target/architecture-validation/` in the implementation worktree. Before/after
counts are preserved for interpreter (190 passed, one ignored), module graph (157 passed), IR
(68 passed), and host runtime (154 passed). Backend/liveness/DB tests total 253 passed and one
ignored both before and after extracting graph tests and moving three DB cases to the oracle.

The initial full `mise run gate` was run once before the follow-up. Evidence is under
`target/gate-logs/run.p98YxxNV/`; `aggregate.json` records a failed verdict, solely for Test262.

| Step | Result |
| --- | --- |
| Release build | passed |
| Workspace tests | 1,992 passed, zero failed, nine ignored; 88 suites |
| Native parity | 222 matched, one existing refusal, zero divergences |
| Thirdparty | 17 matched in each engine; zero divergences; five unavailable bundles not built |
| Thirdparty baseline | no regressions across 13 bundles |
| Facts baseline | no regressions across 31 targets |
| Node / server / regression corpora | 36 / 8 / 15 passed, zero failures |
| Test262 baseline | 119 of 122 chunks completed without count regressions; `staging/sm` allocation abort |

The gate's `staging/sm` log is
`target/test262-baseline-logs/run.ZjfxVRwo/staging_sm.log`. The runner stops at this infrastructure
failure and cannot write a complete Test262 result document. The gate is **not green**.

Independent narrow retries use `scripts/test262.sh --out <report> --path staging/sm` with the
same pinned corpus (`771005236e88a909635104e03ba12559688c0172`) and unchanged default 4,096 MiB cap.
Both the refactor and an independently built clean input abort allocating 536,870,912 bytes and
produce no result document. Wrapper logs are `test262-staging-sm-before.log` and
`test262-staging-sm-after.log` under `target/architecture-validation/`; raw logs are
`target/test262-logs/run.A3U1ZmYg/test262.log` in the baseline worktree and
`target/test262-logs/run.CR3Y2Mex/test262.log` in the implementation worktree.

The two chunks after `staging/sm` were also run separately through the capped runner:
`staging/source-phase-imports` reports zero passed, zero failed and four unsupported, while
`staging/top-level-await` reports zero passed and one failed. Both match their committed counts;
their reports are retained under `target/architecture-validation/`.

This reproduced the initial gate blocker independently of the refactor. No committed baseline,
memory cap, ignored-test list or gate rule was changed to mask it. The subsequent runner fix and
passing gate are recorded in the follow-up report linked above; issue 06 is now resolved.
