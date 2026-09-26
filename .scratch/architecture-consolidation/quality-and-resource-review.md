# Quality, timing and allocation-abort follow-up

Worktree: `/home/alee/Developer/kiln-architecture-consolidation`, based on `30c1d548`.
All measurements below were taken in this isolated worktree before integration, not in the main
checkout. The validated base is recorded above; later upstream commits are not part of these results.

## Code quality

The enforced crate boundaries are the main architectural improvement: module evaluation is
separate from graph construction, the query database receives its analysis query, and native
compilation no longer depends on the interpreter.

Review found that some initial VM files grouped neighboring methods by size rather than by
responsibility. Those methods now have coherent homes for calls/eval, async generators, module
loading, async context, DataView, Date, collections, weak references, disposal, error handling,
closures, process/arguments objects and construction. Misattached documentation was corrected.
All 1,546 interpreter functions are preserved by the source-body audit, with only the previously
reviewed formatting and source-scan-test differences. No Rust source exceeds 5,000 lines.

The review also removed an unnecessary size-provider trait/forwarder: the data was always neutral
instruction/function counts, not C byte measurements. Liveness reads the neutral analysis directly,
preserving the simpler original two-argument APIs. The summary-query registration contract now
documents absent analysis, memoization and deterministic-provider requirements.

Remaining work is explicit rather than hidden: the VM modules still cooperate through one `Vm`
and broad crate-root imports; they are not independent services. Narrowing those internal
interfaces is a separate behavioral-risk-bearing change. The native graph emitter also remains
a substantial mixed planning/emission module despite being below the size ceiling.

## Allocation abort

The default Rayon pool used 32 workers on this host. `KILN_JOBS=4` bounded native script fan-out
but did not bound the interpreter pool inside each Test262 process. A 4 GiB capped process could
therefore overlap large per-case allocations and worker stack reservations until an allocation
aborted the entire chunk. Both unchanged HEAD and the refactor reproduced the abort.

Every immediate `staging/sm` subdirectory completed separately. Running the whole chunk with four
workers and the same cap also completed, confirming that concurrency was the relevant control.
The runner now sets the child Rayon worker count to the minimum of available CPUs, positive
`KILN_JOBS` / `RAYON_NUM_THREADS` settings, and one worker per GiB of configured cap (at least one).
The per-worker allowance is a conservative scheduling policy, not a per-test allocation guarantee.
The hard address-space/RSS cap remains unchanged. No test, baseline or failure was suppressed.

The focused runner tests cover worker limits, invalid overrides, inherited environment preservation
and propagation into the child. `python3 scripts/test262_corpus_test.py`: 10 passed.

## What the timing evidence establishes

Logs and reports are under `target/allocation-investigation/`.

| Measurement | Before | After | Interpretation |
| --- | --- | --- | --- |
| Backend normal dependency closure, unique package/version pairs on this host | 266 | 167 | Fewer required packages; interpreter and host runtime no longer in this closure |
| Interpreter tests, reported execution time, three warm samples each | 0.03 s | 0.03 s | 190 passed and one ignored in every sample; final regrouped suite also 0.03 s |
| Warm `mbx test` command median, including wrapper/Cargo/doc-test overhead | 0.473 s | 0.501 s | No rebuilds; individual samples span 0.458–0.922 s, so not evidence of a meaningful speed change |
| `staging/sm`, equal four-worker concurrency and unchanged cap | 23.684 s | 23.493 s | Identical 1,985 passed / 611 failed / 35 unsupported outcomes |
| Completed default bounded-runner `staging/sm` peak RSS | — | 1,127,988 KiB | About 1.08 GiB; the previous unbounded-worker run aborted |
| Final full gate, `KILN_JOBS=4` | no comparable completed measurement | 10m 12.33s | Includes build, crate tests, native parity, bundles and complete Test262 baseline |

These are not clean-build benchmarks. Earlier build logs have different cache-hit counts and
cannot support a before/after compilation-speed claim. Splitting files inside one Rust crate does
not itself create additional compilation boundaries. Removing dependency edges does; the closure
comparison establishes that structural benefit without inventing a wall-time improvement.

The bounded runner trades unconstrained per-process parallelism for predictable memory use and
respect for the requested job budget. Whole-corpus timings from 32 uncontrolled workers should not
be compared directly with a four-worker run as if both used the same resources.

## Final validation

The full gate was run once after the follow-up changes were finished:
`target/gate-logs/run.TgSsEtVC/aggregate.json` records **passed**, all ten steps.

- Workspace: 1,992 passed, zero failed, nine ignored, 88 suites.
- Native: 222 matched, one existing refusal, zero divergences.
- Thirdparty: 17 matched in each engine, zero divergences; five unavailable bundles not built.
- Node/server/regression corpora: 36/8/15 passed, zero failures.
- Test262: all 122 baseline chunks completed, no regressions; 96,890 passed, 5,362 failed,
  382 unsupported and four skipped. Existing failures are baseline outcomes, not a claim of
  complete Test262 conformance.
- Formatting, diff checks and the layer test pass. The committed Test262 baseline is unchanged.

The gate stream and resource timing are preserved in `target/allocation-investigation/gate-fixed.log`.
Source audits, the paired timing samples and narrow capped-runner reports are retained alongside it.

## Pre-commit checks

Formatting, diff checks and the ten Python runner tests pass. The additional strict check,
`mbx clippy --workspace --all-targets --locked -- -D warnings`, stops at existing style lints in
`kiln-host-abi` and `kiln-module-resolver`. Both source files are byte-identical to their original
locations at the input commit; the host-ABI failures also reproduce in the clean baseline worktree.
The logs are `precommit-clippy.log` and `precommit-clippy-baseline.log` under
`target/allocation-investigation/`. These unrelated lints were not folded into the refactor.

Before committing, `origin/main` had advanced to `fa213915` with fourteen additional commits,
including overlapping runtime changes. The passing gate above validates the recorded input base
plus this change, not that newer upstream tree. Integration with newer main requires a separate
conflict review and validation; it must not be treated as already tested.
