# Consolidation validation record

This is a local validation record for the consolidation tree. It records one full gate and the
focused follow-ups that closed its findings; it does not claim a second full-gate run.

## Full gate and closures

The single full gate was `target/gate-logs/run.UumBMiKa/`. Its Test262 lane reached chunk 120 and
then stopped at `staging/source-phase-imports` because the runner treated Kiln exit 1 with four
unsupported and zero failed cases as an infrastructure error. The shared corpus policy now accepts
that normal unsupported-only status while still rejecting signal exits and other abnormal statuses.
The affected chunk and the remaining `staging/top-level-await` chunk were rerun individually:

- `.scratch/test262-staging-one-v2/run.log`: `staging/source-phase-imports`, 0 passed, 0 failed,
  4 unsupported, no regression.
- `.scratch/test262-staging-top-level-await/run.log`: `staging/top-level-await`, 0 passed, 1 failed,
  no regression.
- The gate's first 120 measured rows total 96,766 passed and 5,485 failed; adding the two staging
  rows gives 96,766 passed and 5,486 failed across 122 chunks. These are measured gate rows, not
  values copied from the committed baseline.

The workspace rerun after the fixes passed 80 suites, 1,765 passed, 0 failed, and 7 ignored; its
record is the `kiln-require-resolve-cleanup` workspace log. The full gate was not rerun after this
closure.

## Focused acceptance

- Native parity: 195 matched, 1 intentional ShadowRealm refusal, 0 divergences.
- Third-party: 17 interpreted/native matches, 0 divergences, 5 absent.
- Conformance: 36 Node tests, 5 server tests, and 15 regression tests passed.
- Test262: the measured 122-chunk totals above, with zero regressions in both staging reruns.
- Performance: 13 comparisons reported no regression; this record makes no speedup claim.
- The final CLI SHA-256 is `4f8a4efefbf293c20295704fdb11ccbec9c85fcd13aa1ffb19039ca3d96160a2`;
  the direct resolve probe passed.
- The provider checker exited 0; complete log:
  `.scratch/provider-check-shared-final-v2.log`.
- The shared script-hygiene target passed 6 tests; log:
  `.scratch/test262-script-hygiene-final.log`.

These focused checks and their logs are the evidence used after the one full gate; no claim here
replaces the gate's original output or treats a partial rerun as a new full gate.
