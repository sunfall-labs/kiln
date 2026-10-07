# Native instantiation tier milestone

The code candidate `f4a22dbbb9bd3df1a2322637d94629232229b5e6` passed the routine repository gate: all ten steps, terminal exit zero, 1,182 seconds. Frozen CLI SHA-256: `35fc543aea252fd0b1e8160b6f2871f5e58326f970905cb6836319e5746eddf2`. Tested tracked-tree snapshot: `c28abed86bdba732de483df6025bf7bdaa598d6af5b25726d726581d9a69043c`. Code was pushed in `f539a24c`, which merges public-main documentation only. The tracked-tree snapshot includes documentation, so that digest changes after the merge; a direct diff confirms only two documentation artifacts changed, and the production CLI hash is unchanged.

The compiler now preserves mixed optimization metadata through serialized declaration-instantiation units and both executable/shared-library builders. Selected bodies receive actual O1 compilation and cold bodies keep their O0 attributes; compiler options remain part of native cache identity. The bounded known loop-callee group selector preserves loop ranking across module phases and excludes unavailable callers. It retains four bodies per group, eight supplemental module bodies and 4,096 supplemental instructions. The separate broader-budget experiment is excluded.

Two private runtime optimizations accompany the compiler fix: header-adjusted string candidates reuse nursery-region membership only inside the same region, with integer-address bounds and an authoritative fallback; ordinary string keys skip hidden-key-name comparisons while reserved domains retain exact checks. These do not introduce new C built-ins.

Verification:

- Workspace: 2,746 passed, zero failed, 12 ignored across 110 suites (2,758 tests). Backend 353 passed plus 37 emitted-C integration tests are included in this total.
- Native corpus: all 668 probes accounted for; 663 matched, five existing explicit refusals, zero divergences.
- Third-party correctness: all 17 present bundles matched in both engines; zero native refusals or divergences; five absent bundles explicitly reported.
- Routine Test262: 97,694 passed, 4,440 failed, 384 unsupported, four skipped; exactly 102,522 selected, 116 deferred, 102,638 discovered. All 122 chunk floors pass with no regressions. This is routine baseline verification, not full language conformance.
- Node 40, server seven plus two skips, regress 58 and facts 31 all pass.
- All 12 routine performance bundles pass the unchanged 25% plus 3 ms policy; postcss is deferred. The gate waited for correctness before serial warmup/measurement (4.60 seconds). Separate agent builds were held. This is not the final 23-microbenchmark/four-realworld Bun comparison.
- After the gate, the combined frozen CLI matched both affected native probes under allocation stress (`owned-string-boundary-gc.js`, `async-nursery-pressure.js`): two matched, zero refused/diverged/skipped.

Disk recovery retained complete gate output and verdicts. Only this worktree's materialized probe executables with terminal results and no active executable users were removed. The ordinary cache collector retained grace/in-flight protections and compile locks; the finished workspace fixture cache was collected through an isolated project pointing at that owned cache. Full recovery receipts are archived.

The complete Effect goal remains unfinished. Earlier original replays were Hash 24/25, SchemaBinary 192/193, ClusterWorkflowEngine 46/47 and Sharding 61/66; they do not prove acceptance of this new combined compiler milestone. Fresh original-case replays on this frozen CLI are next. Full passing native/interpreter surveys of 322 files and 11,019 original cases, and final quiet Bun comparisons, remain required. Original assertions, skips, case identities and five-second deadlines are unchanged.
