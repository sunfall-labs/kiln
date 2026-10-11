# Targeted async-generator Test262 diagnostic (2026-10-08)

This is a diagnostic run of three complete Test262 subtrees on source `945ebdac762688419cc9cfa39b90966b6fdd4043` (tree `dcf30de2c1c55fe8f9f6f7ed9545a46a127e0ec7`) and frozen CLI SHA-256 `cbc46072e59416d5b3fee5704ee85d7e437218e0163fb1712ea9863e4c6700ae`. The Test262 checkout was clean at pinned commit `771005236e88a909635104e03ba12559688c0172`. This is not a full Test262 run or a gate result.

The sequential command for each row was `scripts/test262.sh --out <fresh-output.json> --path <subtree>`, using one worker, 4096 MiB, the runner's original test deadlines, and a 600-second per-subtree supervisor bound. All 218 selected cases ran, each selected inventory hash exactly matched the pinned manifest, and the runner reported no unsupported or skipped cases. All process leaders were reaped and process groups absent. The raw `status.json`, three result JSON files, three internal runner logs, and parsed failure details are retained in `raw/`.

| Routine subtree | Cases | Diagnostic | Full-baseline counts | Pass delta |
| --- | ---: | ---: | ---: | ---: |
| `built-ins/AsyncGeneratorPrototype` | 96 | 76 pass / 20 fail | 62 pass / 34 fail | +14 |
| `built-ins/AsyncGeneratorFunction` | 46 | 40 pass / 6 fail | 38 pass / 8 fail | +2 |
| `built-ins/AsyncFromSyncIteratorPrototype` | 76 | 64 pass / 12 fail | 64 pass / 12 fail | 0 |
| **Total selected** | **218** | **180 pass / 38 fail** | — | **+16** |

These three paths are *not* entries in the extended-suite `chunks` list. They are present in its inventory only, so the routine per-chunk floor is the full baseline count shown above. The committed baselines contain pass/fail totals, not per-case historical outcomes; the deltas are count comparisons and do not identify which individual cases changed. `raw/baseline-subtrees.json` records the manifest hashes and distinction.

## Failure groups

`raw/failure-details.json` lists every failed case ID and its observed assertion/runtime diagnostic.

* **AsyncGeneratorPrototype (20):** two constructor descriptor failures; twelve invalid-receiver cases across `.next`, `.return`, and `.throw` (sloppy/strict forms, currently surfacing as uncaught “not a function” TypeErrors); and six broken-promise `.return` cases. Four broken-promise variants report “Expected rejection”; the suspended-yield try/catch variants instead fail reading `message` from an undefined caught error.
* **AsyncGeneratorFunction (6):** two cross-realm prototype-identity failures, two `.prototype` non-callable length assertion failures, and two `.prototype` descriptor configurability failures.
* **AsyncFromSyncIteratorPrototype (12):** six poisoned Promise-wrapper tests expected rejection but observed no exception; six rejected yielded-promise tests expected iterator `finally` to run once but observed zero.

The strongest next fix hypothesis is the generic async-from-sync delegation completion path. The poisoned-wrapper tests exercise abrupt `PromiseResolve` while wrapping an iterator result and expect the promise to reject; the close tests exercise a rejected yielded value and expect the underlying sync iterator to be closed. A single spec-aligned abrupt-completion/IteratorClose path could address both groups (12 cases). This is a source-based hypothesis from the test descriptions and observed assertions, not a proven code diagnosis. The separate invalid-receiver and constructor-property groups should remain separate fixes.

## Return-operand control

The prior c1 CLI report at `raw/controls/prior-c1-AsyncGeneratorPrototype.results.json` has the same six broken-promise `.return` case IDs failing, including the suspended-yield try/catch cases. Its target CLI digest is recorded in `raw/controls/prior-c1-control.json`. Thus the current results do not show those failures as newly introduced relative to that prior run. The c1 release receipt pins that earlier run to source commit `624968060241897edfc7d38b40f18aae80a3d2bc`; the active return-operand fix `913f0f4a48f409a55885cc62f627a92ce996fd01` is present in current source `945` and is not an ancestor of `624`. The same six exact IDs failed on both sides, so they are not newly introduced by the fix; the fix has not resolved them. This is not a one-variable experiment because other source changes also differ. The current source contains the active async-generator return-operand fix, but those six Test262 cases remain unresolved here.

## Evidence files

`raw/portable-receipt.json` records source, CLI and corpus pins, resource bounds, inventories and process cleanup. `raw/status.json` preserves the original supervisor receipt, including machine-local paths. `raw/SHA256SUMS` hashes all evidence files under `raw/` except itself. This README is outside that evidence manifest. No CLI binary, build cache, or mutable upstream test file is included.
