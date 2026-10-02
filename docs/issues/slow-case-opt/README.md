# Deferred-test optimization integration

The initial build-loop and routine/extended split was pushed on main at `80f55d78`.
This follow-up uses three owned agent worktrees and integrates their work with upstream
`0c067611`, including the new common bootstrap in every interpreter realm.

## ADR review

The proposed broad per-object property hash index was withdrawn before implementation.
ADR 0050 rejects that native object model in favor of shared shapes, and ADR 0071 keeps
shared caches about layouts, paths and code rather than objects. The interpreter/native
representation distinction is not an approval for replacing the agreed architecture.

The retained constant index exists only during compilation and preserves constant identity
and first-use order. Temporary ownKeys membership sets preserve the ordered result.
String copying and guarded numeric operations are representation optimizations, reusing
existing semantic helpers with generic fallbacks. No C built-in, heap layout, cross-layer
dependency, execution budget or baseline relaxation was added. The array follow-up removes
a legacy descriptor bypass and reuses the existing descriptor/setter authorities, consistent
with ADR 0060. Future mutable intrinsic prototype links must invalidate the numeric guard.

## Measurements and diagnosis

Controlled agent comparisons against the earlier `80f55d78` baseline:

| Work | Before | After | Result |
| --- | ---: | ---: | --- |
| 448 Unicode/RegExp files | 166.192 s | 125.406 s | All 896 variants pass; 24.5% less wall time |
| 48 mixed files | 30.930 s | 15.424 s | Exact 96 outcomes unchanged |
| Casing table, repeated median case | 11,576.53 ms | 707.46 ms | 16.36x, indexed constant interning |
| Proxy ownKeys, repeated median case | 2,307.63 ms | 1,389.62 ms | 1.66x, temporary membership sets |
| Three Map/Set iterator-close files | 518–1,503 ms per variant | 0.44–0.92 ms | Six failures become passes; honor overridden adders |

The complete group measurements are one pair each, subject to shared-host load; the focused
medians use three alternating pairs. See the [Unicode](../extended-unicode/evidence.md),
[mixed](../extended-mixed/README.md), and [collection/TypedArray](../extended-limits/REPORT.md)
reports for selection, exact outcomes, executable identities and reproduction.

A fresh 12-file/24-variant comparison against latest upstream `0c067611` took **21.468 s
before and 2.979 s after**, with **12 to 18 passing variants**, no passing regressions and
identical IDs. This is one representative pair, not a whole-gate speedup claim.
[Case timings](latest-focus-comparison.json), [selection](current-focus.ids.json), and
[candidate identity](final-binary.json) are retained. Both runs used four workers, the capped
runner, 4096 MiB memory and the original corpus. The candidate is commit `cbb91194`.

Latest upstream changes the TypedArray diagnosis: all six selected variants now fail promptly
on the shared TypeScript copyWithin implementation's missing post-coercion detachment error,
on both upstream and candidate. This is not an optimization win. The older report's finite
byte-copy setup exhausting 8M steps describes the earlier, pre-bootstrap implementation.
Numeric TypedArray storage operations were improved, but these conformance failures are unresolved.

The mixed inventory also identifies a small for-of/try-finally/iterator-close case that loops
incorrectly; it is not inherently expensive. Large finite URI, calendar and regexp sweeps,
surrogate-source refusal, timezone enumeration and absent GC harness support remain separately
documented. Time to an instruction limit is never presented as completion time.

The three fixed Map/Set files return to routine coverage: **101,640 routine variants**, with
**499 files / 998 variants** retained in extended coverage. No pinned test was deleted or edited.

## Correctness and validation

Two failures encountered in the earlier gate were also fixed: watcher test fixtures now publish
completed edits atomically, and native boxed integer remainder preserves NaN and negative zero.
Both have focused regression evidence. The [inherited-array follow-up](../array-prototype-writes/REPORT.md)
fixes strict/sloppy inherited descriptors and setters, with 978 affected Test262 outcomes unchanged
against latest upstream and a new regression that fails upstream and passes candidate and Node.

`mise run gate-full` passed all ten stages at `cbb91194` in **386 seconds (6m26s)**:

- 2,058 crate tests passed, zero failed, nine ignored.
- Full Test262: 102,638 variants; 97,081 passed, 5,169 existing failures, 384 unsupported,
  four skipped. No regressions across 122 chunks; 43 gained passes. This stage took 99 seconds.
- Native parity: 244 matched, one existing refusal, zero divergences; 261 seconds.
- Thirdparty: 17 interpreted and 17 native bundles matched; five not built, zero divergences.
- Node, server and regression corpora, performance baseline and facts baseline passed.

[Retained summary](gate-summary.json); full local evidence is in
`target/gate-logs/run.aCBrTGxK/aggregate.json` and `docs/issues/slow-case-opt/final-gate.log`.
The summary names the exact validated source snapshot and gate executable digest. Documentation
added afterward changes the tracked snapshot; it is not part of that measured source identity.

The complete gate still exceeds the requested few-minute ceiling. Workspace tests including
compilation took 339 seconds; native parity took 261 seconds concurrently. Native compilation
and the workspace native tests are now the main remaining build-loop costs. This run includes
build/cache effects and cannot be compared directly with an earlier warm gate as a controlled
whole-gate speedup. No tests, assertions, execution limits or baselines were weakened.

While the gate ran, main advanced to `0da8a1c4` with native realm/collector/method-cache changes.
The merge has zero shared edited files with this optimization bundle and changes no interpreter,
constant interning, string storage or Test262 baseline/classification code. The full gate above
validates the premerge native runtime. Focused postmerge evidence is recorded separately below.

At merged commit `49cb3bcc`, all three focused backend tests passed (remainder boxing, method
shape/code stores, and two-realm module globals). Five native probes matched with zero refusals
or divergences, both normally and with `KILN_NATIVE_GC_STRESS=1`: numeric remainder, method
prototype cache, prototype mutation paths, array insertion/rooting, and shape-key collection.
[Merge validation and executable identity](merge-validation.json). These targeted checks validate
the merge interaction; they are not a second complete gate.
