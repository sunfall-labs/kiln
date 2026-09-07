# Ghui codegen and build-latency improvements

Worktree: `/home/candrewlee/Work/kiln-ghui-perf`, branch `investigate/ghui-perf-opentui`, based on `feat/node-ffi-opentui` at `0936620ea7bb24ca59ce9b21da917ea3dd30750b`. The original ghui checkout remains untouched. The copied ghui source and toolchain are described in REPORT.md.

## Implemented

1. Remove unused context/receiver setup from compact boxed function bodies.
2. Remove dead phi chains and cycles without dropping effectful operations.
3. Resolve trivial phis, including loop-invariant self-input phis, to their single effective input. Preserve all positional suspension/resume phis.
4. Renumber remaining SSA values densely, remapping operands, returns, branches, throws, suspended values/stacks, initial-local references and diagnostic origins before escape analysis. Preserve call-site and bytecode IDs.
5. Cache ordinary source/CommonJS module emission before graph building and C rendering, for split-prelude graph builds. Keys cover the complete owned module IR, source kind, logical path, full emission policy, target, ABI, generator epoch, prelude header and actual emitter/wrapper source fingerprints. Builtin materializers, JSON and embedded-runtime emission bypass this new stage.

6. Schedule larger module C compilations first using the existing Rayon CPU budget. Preserve original result/link ordering, deterministic lowest-index errors and panic handling.

The cache uses a versioned internal encoding of the complete derived Debug representation of ordered IR/policy fields, hashed with SHA-256. It does not key on mtimes or just JavaScript text. The payload is a validated cache record containing the generated C and its symbol/program metadata. An emitter-source fingerprint generated at Rust build time prevents dirty backend-source edits from being hidden by a cache hit under an unchanged generator epoch. The native wrapper source is fingerprinted too.

## Output quality

| Metric | Original | Final | Reduction |
| --- | ---: | ---: | ---: |
| Application C bytes | 94,208,942 | 84,177,338 | 10,031,604 (10.6%) |
| Application object bytes | 104,236,224 | 94,344,600 | 9,891,624 (9.5%) |
| Retained cold-build executable bytes | 73,058,464 | 64,837,040 | 8,221,424 (11.3%) |
| Emitted phi copies, including module entries | 367,010 | 20,813 | 346,197 (94.3%) |
| Phi-copy source bytes | 8,718,991 | 991,836 | 7,727,155 (88.6%) |
| `treeSitterToTextChunks` SSA slots | 8,782 | 1,961 | 6,821 (77.7%) |
| `treeSitterToTextChunks` stack reservation | 208,536 | 29,096 | 179,440 (86.0%) |

The example function's C body is now 218,670 bytes, versus approximately 453 KB before cleanup. The earlier identity-function cleanup reduced its actual code from 238 to 114 machine-code bytes.

Paired warm-build executable artifacts are 73,070,808 and 64,853,488 bytes. They have the same application-object totals as the retained cold artifacts above, but slightly different link contexts; do not mix those executable sizes between comparisons.

## Warm build latency

Alternating baseline/current/current/baseline builds after priming, with no C compilation or linking in the timed warm rows:

| Compiler | End-to-end wall seconds | CPU seconds |
| --- | --- | --- |
| Original branch | 4.453, 4.395 | 6.544, 6.497 |
| Current, with emission cache | 3.439, 3.461 | 3.836, 3.756 |

Mean wall time improved from 4.424 to 3.450 seconds (22.0%); CPU consumption decreased about 42%. Before the emission cache, codegen cleanup alone did not establish a warm wall-time win: baseline 4.365/4.209 s versus cleanup 4.488/4.208 s. This distinguishes cache latency gains from structural code-quality gains.

The final source-fingerprint guard changes invalidation identity without changing generated C. Cache tests passed after that guard was added. Cold-object replay below uses the final binary including this guard.

## Cold-object build latency

Application-object and executable caches were moved aside for each run and restored afterward. Generated-source, runtime/prelude and Rust number-provider caches stayed warm. Each row therefore includes module build orchestration, application C compilation and linking, but excludes a fresh Rust-provider bootstrap. Baseline/current/current/baseline order, identical source project and compiler flags, eight Rayon workers:

| Compiler | End-to-end wall seconds | CPU seconds |
| --- | --- | --- |
| Original branch | 22.981, 22.799 | 81.375, 100.397 |
| Final, including larger-first scheduling | 17.825, 18.243 | 91.074, 86.624 |

Observed mean wall time: 22.890 → 18.034 seconds, **21.2% lower**. A separate checkout was running a Rust release build during this comparison, so this small ABBA sample is evidence for this measured workload, not a universal speedup guarantee. Before queue scheduling changed, an earlier ABBA replay gave baseline 17.983/22.820 s versus cleanup/cache 20.106/20.081 s; it did not establish a cold wall-time improvement.

All 145 generated native-object cache records and the linked-artifact record are byte-identical before and after the scheduling change. Thus scheduling changes when work starts without changing object contents or link order. Both final full ghui builds succeeded.

Records: `results/cold-object-replay-scheduled/results.json` and per-run logs in that directory. Earlier comparison: `results/cold-object-replay/results.json`. Original application caches were restored; all replay artifacts are retained only in this owned worktree.

## Validation

- Backend library: 117 passed, zero failed, one ignored.
- Reviewed/updated two C snapshots for removed copies and dense value numbering; all 34 snapshots other than the two preexisting mismatches pass.
- Native regression fixture covers parallel rotations, discarded getter/coercion effects, throws/finally, captured bindings, generators and async suspended operands. Native execution agrees with Node.
- Emission-cache test proves persistent hits skip the emitter, and checks invalidation across code, source text, strictness, exports, linking facts, source kind, logical path, target, compact mode, realm policy, type profiles and export liveness. Embedded-runtime bodies bypass it.
- Remaining workspace: 72 suites, 1,330 passed, zero failed after reviewing the facts baseline.
- Facts baseline: only 31 `Unknown` return types became concrete (21 Object, four Boolean, three Number, two Callable, one Undefined). No other baseline fields changed. Source checks support the stronger facts; updated baseline passes all 31 targets.
- Third-party parity: 17 available bundles matched in each engine, no divergence; five bundles absent. Performance baseline passed all 13 available bundles.
- Node/server/regress corpora: 35/5/12 passed, zero failures.
- Full gate completed Test262: 122 chunks, 96,412 passing, no baseline regressions.
- The full gate's facts-baseline failure was resolved by the reviewed baseline update and passing targeted recheck. Two C snapshot mismatches and signal exits in `typed-callbacks.js` and `exceptions.ts` remain; they were reproduced on the untouched base branch earlier. See IMPLEMENTATION.md and the base-comparison logs. The branch is not represented as having a green full gate.
- After the final scheduling change, all 109 kiln-native library tests passed (two ignored), including priority order, original result ordering, lowest-index error selection, worker panics, and real executable/shared-graph builds. This narrow follow-up and the byte-identical ghui object/link replay validate the scheduling-only delta after the full gate; Test262 was not rerun for a queue-order change.
- `git diff --check` passes.

## Stopping point

These changes remove demonstrably redundant generated work, make repeat builds faster, and reduce the measured cold-object build tail through scheduling. Remaining major C size comes from the retained library/function population and generic operation/bridge lowering, documented in CODEGEN-AUDIT.md and C-QUALITY.md. Further substantial reductions require broader liveness, representation or helper-lowering changes with greater semantic risk. No global O1 switch or speculative elimination of JS effects was added.

Ghui's preexisting OpenTUI initialization failure still prevents a valid interactive throughput/frame-rate comparison. The stack/copy reductions are real, but are not reported as an application frame-rate gain. A working interactive baseline and profile should precede the next substantial runtime optimization.

Logs and retained artifacts from the investigation are under `results/` and `tmp/` in the original performance worktree. The measurements above compare the original feature-branch base with the performance changes; the current-main validation below is separate.

## Mainline integration, 2026-09-07

The prerequisites were already on remote `main`. Applied the remaining performance commit to `41c4e22e` in the owned `kiln-ghui-land-main` worktree, producing `ff2c6c12`. No prerequisite commits were duplicated. Current main also contains fixes for the earlier snapshot and native-parity failures.

Validation of this exact integration:

- Workspace: 75 suites, 1,485 passed, zero failures.
- Native parity: 157 matched, seven expected refusals, zero divergences.
- Both new regression fixtures also matched with `KILN_GC_STRESS=1`.
- Third-party parity: all 17 available bundles matched in both engines; five bundles absent.
- Facts baseline: all 31 targets passed. Node/server/regress: 35/5/12 passed.
- Test262: 96,412 passing, no regressions across 122 chunks, six chunks gained.
- Full gate failed only the historical third-party timing baseline, on postcss, rxjs, semver and vue-reactivity. The committed timing baseline was not changed.

To distinguish existing mainline timings from a patch regression, built the same four release bundles using untouched main (`41c4e22e`) and the integration (`ff2c6c12`). After the gate and all builds finished, ran eleven baseline/current/current/baseline rounds, yielding 22 samples per version per bundle after five warmups. Every stdout matched Node. Median whole-process milliseconds:

| Bundle | Untouched main | Integration |
| --- | ---: | ---: |
| postcss | 15.810 | 14.808 |
| rxjs | 21.194 | 21.170 |
| semver | 11.717 | 11.733 |
| vue-reactivity | 6.655 | 6.576 |

All four already exceed the historical thresholds on untouched main. This comparison shows no material additional slowdown from the patch; it does not resolve main's existing runtime-performance issues. See `.scratch/native-perf/thirdparty-baseline-stale.md` and `.scratch/native-perf/release-lost-its-optimisation.md`.

Full logs, the comparison script, individual samples and retained binaries are in the original performance worktree's `results/current-main-gate-short.log` and `results/main-bundle-comparison/`. Two earlier validation attempts were invalidated by scratch storage and Unix socket path constraints; the completed run used the owned short scratch path `/home/candrewlee/kg.4eIMhy` and a 6 GiB memory / 4 GiB swap cap.
