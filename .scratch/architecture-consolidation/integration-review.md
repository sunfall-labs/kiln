# Upstream integration review — 2026-09-26

Review branch: `refactor/architecture-consolidation`.

The allocation fix and architectural refactor were committed separately as `2e3866e4` and
`f7a8ee6f`. The branch then incorporated `origin/main` at `fa213915`, fourteen commits beyond
the original `30c1d548` input. This review covers that integration; the earlier verification
documents retain the measurements of the original input and must not be read as measurements
of the newer upstream code.

## Review and conflict resolution

Three Luna reviewers divided backend/native, interpreter/architecture, and runtime/provider
integration. A separate tools/protocol/script review found no migration defects. The final
architecture review found no blockers in the neutral graph extraction, summary-query ownership,
module-evaluation boundary, dependency-direction tests, or bounded Test262 runner.

Upstream changes were ported into their new owning modules rather than restoring the old flat
files. This includes extension filtering, negative-zero rounding, parallel-site reporting,
process supervision, shared keys/shapes, and executor/realm ownership. A token-aware audit of
upstream-added or changed Rust function bodies found exact preservation for 21 interpreter,
45 runtime, and 22 provider function names. The eight native and 96 backend names differ only
in two expected neutral-analysis paths and an omitted comment inside one C test fixture.

The expanded native launcher would have pushed `graph.rs` over 5,000 lines. Shared-library
wrapper generation now lives in `shared_graph.rs`, and its source participates in the emission
fingerprint. Every workspace Rust source remains below 5,000 lines.

Compilation caught and corrected provider merge syntax/borrow-scope mistakes and visibility
needed by the extracted wrapper module before the final gate. The corrected provider function
bodies match upstream, including fallible borrowing and explicit borrow-release points.

## Existing upstream findings

The native reviewer identified two issues already present, unchanged, in `fa213915`:

- On non-Linux systems, forked workers retain the supervisor death pipe's write end, so the
  reader cannot observe EOF after abrupt supervisor death. See `kiln_sup_worker` and its fork
  call in `crates/native/kiln-native/src/graph.rs` (upstream flat `lib.rs` around lines 6875
  and 6977). Linux uses `PR_SET_PDEATHSIG` instead.
- Overflow-sized positive `KILN_WORKERS` strings parse differently: the Rust parser falls back
  to one worker while the generated C parser clamps to 1,024. Existing tests cover `2000`, not
  integer overflow.

These are not refactor regressions and are not changed here. They need separate supervisor
follow-up, including macOS/BSD execution; this worktree's native validation runs on Linux.

## Validation

The integrated tree passes `mbx check --workspace --all-targets --locked`, the two layer tests,
the negative-zero regression, the two-heaps shared-key/shape regression, and all ten Python
Test262 runner tests. Evidence is under `target/allocation-investigation/integration-*`.
The full gate passed all ten steps in 13m 01.84s. Its aggregate is
`target/gate-logs/run.mUHV3H5h/aggregate.json`: 2,031 crate tests passed, nine ignored;
234 native matches, one existing refusal, no divergences; 17 thirdparty matches per engine
(five bundles unavailable); node/server/regression totals 36/8/15. All 122 Test262 chunks
passed baseline comparison: 96,892 passed, 5,360 existing failures, 382 unsupported, four
skipped. The committed baseline is unchanged. The complete HTTP-close contract also passed.

The extra GC-stress run completed 231 matching probes and one refusal, with three newly
upstream allocation-heavy parallel fixtures still running after twelve minutes. It was stopped
to prioritize integration at the user's request; this is not a completed stress-suite pass.
Partial evidence is retained under `target/allocation-investigation/integration-native-stress*`.

Main advanced again to `04e460a0` during validation. This gate validates the integration through
`fa213915`, not those three newer commits; they require a separate integration check.

## Final landing integration

At the user's request to prioritize landing and avoid another broad testing cycle, the branch
also integrated main through `f93847a0`. Three Luna agents resolved/reviewed the split-module
conflicts, structured-clone/transfer bootstrap changes, shape-cache changes, and worker-thread
surface updates. All fourteen changed backend and both added runtime function names retain
upstream bodies; the twelve changed interpreter names differ only in the order of three
disjoint native-name match arms. The dispatcher fragments concatenate byte-for-byte to this
upstream version. Every Rust source remains below 5,000 lines.

The focused landing checks pass: release CLI build, 174 backend unit tests, all 36 emitted-C
snapshot tests, and 190 interpreter tests (two existing ignored tests across the crate runs).
Logs are `landing-release.log` and `landing-crate-tests.log` under
`target/allocation-investigation/`. The full gate above was not repeated for this last merge;
its evidence remains tied to the earlier integration, not silently relabeled as the latest tree.

The three new native probes (message-port transfer, worker-thread refusals, detached typed-array
mapping) all match under `KILN_GC_STRESS=1`, including the parallel probe's requested worker
counts. All 17 regression-corpus tests pass with `--node`. Their logs are `landing-native.log`
and `landing-regress.log`; the latter also has `landing-regress.json`. No additional full gate
or whole stress sweep was run for this final integration.
