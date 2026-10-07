# Native linker-input CAS and gate measurements

The native builder now stages the validated number-provider archive in a raw,
content-addressed pool beside the first writable artifact-cache layer. On a
link-cache miss it verifies the pooled bytes and hard-links the archive into
the private link workspace. The pool has a 96 MiB steady-state limit and
evicts old digest-named archives without invalidating live workspace links.
Executable/library cache hits never stage the archive. This changes neither
provider selection nor native artifact keys; it is a cache of compiler input
bytes, not of test verdicts or realm objects (ADRs 0062 and 0071).

`measure.py` builds 12 distinct two-module native programs in a fixed project
directory, at most two CLI processes at a time. Each condition receives a
fresh shared native artifact cache. The provider source archive is already
built in both conditions, and the same Clang wrapper, compiler, input files,
one-worker CLI budget and host are used. Every build executed and returned the
expected integer; both versions invoked the C compiler 39 times. The before
binary is from `bf8aab31`, and the after binary adds only the provider CAS
(including the standard-library hard-link fallback). Their SHA-256 hashes are
recorded in each `result.json`.

| Run | Wall | CLI + compiler output I/O | C compiles |
| --- | ---: | ---: | ---: |
| before1 | 11.89 s | 3,435,960 blocks | 39 |
| after1 | 12.62 s | 2,925,912 blocks | 39 |
| after2 | 22.92 s | 2,931,296 blocks | 39 |
| before2 | 14.07 s | 3,453,376 blocks | 39 |

Linux reports these output blocks in 512-byte units. The CAS saved 249–255 MiB
of total output I/O per 12 builds (14.8–15.1%), almost exactly one 23 MiB
archive write per link after initial publication. Each populated pool used
about 23 MiB. The `wait4` counters include the compiler/linker children, so
the block reduction is an observed whole-build result, not a direct byte count
from the CAS. Wall times varied under host contention; **these runs do not
establish a latency improvement**. Case-level status, CPU and I/O evidence is
in `before1/result.json`, `after1/result.json`, `after2/result.json` and
`before2/result.json`.

Three focused native tests passed: same-inode reuse plus corruption rejection,
quota eviction with an in-flight workspace link, and eight concurrent callers
publishing one archive. The release CLI built with `mbx --locked`. The first
candidate build correctly rejected an unnecessary direct `libc` dependency's
lockfile delta; the final change uses `std::io::ErrorKind` and leaves
`Cargo.lock` untouched.

Two CLI HTTP fixtures now use the existing shared native semantic cache; the
test bodies and runtime assertions still run. In the merged full gate, the CLI
unit binary passed 162 tests in 9.47 s. That is a validation result, not a
controlled speedup claim for these two fixtures.

The first merged routine gate hit its 300 s cap while compiling new upstream
native artifacts; Test262 completed in 87 s without chunk regressions. A warm
routine gate then completed in 233 s with one unrelated stale facts-inventory
failure. `bench/results/facts-baseline.json` was regenerated on the merged
source: exactly nine of 31 targets changed, only in site facts. Their world
verdicts, module/function counts, types, purity and throw flags are unchanged.
The focused facts check passes.

The final `gate-full` passed in 263 s: 2,148 workspace crate tests passed
(9 ignored); native parity 310 matched / 1 refused / 0 diverged; all 102,638
Test262 variants selected with 97,183 passing and no chunk regressions;
thirdparty and the node/server/regress corpora passed. Step timings were
crate tests 257 s, native parity 110 s, complete Test262 130 s, and
thirdparty baseline 4 s. The native crate binary took 199.41 s during this
gate because of cold artifacts; an isolated warm repeat took 41.20 s for
237 passing tests (2 ignored), plus 4 passing surface-drift tests.

`scripts/gate.sh` now gives the post-correctness thirdparty baseline build
phase its released CPU budget: independent bundles each run one-worker CLI
builds, capped at 16 outer workers. The actual serial benchmark timing loop
is unchanged. The observed baseline step fell from 84 s in the preceding
gate to 4 s in the full gate, but the artifacts were warm on the latter run,
so that difference **cannot be attributed to the scheduling change**.

The resource watcher saw no hang or memory pressure during these runs. The
four isolated benchmark caches (about 1.37 GiB together) and both copied
binaries (about 465 MiB together) were removed after retaining their result
documents and binary hashes. `kiln --one-shot cache gc` reclaimed 1.2 GiB
from this worktree's native artifact cache; 6.9 GiB remains protected by its
grace period. The last gate and warm native check left no child processes
running. Raw, uncommitted logs and gate envelopes remain in
this worktree under `merged-gate/`, `warm-gate/`, and `final-full-gate/`.

After that full gate, main added interpreter `flatMap` and native arithmetic
branch fixes. They were merged without conflict. The first exact-tree routine
gate reached its 300 s cap during cold native tests; every completed lane
passed and no child process survived the timeout. Repeating the same tree
warm passed the complete 10-step routine gate in 285 s: 2,150 crate tests
passed (9 ignored), native parity 311 matched / 1 refused / 0 diverged,
routine Test262 96,233/101,640 passing with no chunk regressions, and all
thirdparty, corpus and baseline checks passed. The steps took 200 s for crate
tests, 160 s for native parity, 49 s for Test262 and 82 s for the thirdparty
baseline. The latter remains a cold-link tail even with 13 bounded outer
workers; this work does not meet a sub-three-minute cold-gate target.
The watcher observed at least 26.8 GiB available memory during the 13-build
phase, no sustained I/O stall, and no orphaned processes. Exact-tree raw
evidence is under `latest-routine-gate/` and `latest-routine-gate-warm/` in
this worktree.

## Follow-up: isolate the PostCSS benchmark and narrow graph emission invalidation

The baseline step above lacked per-bundle build timings. The follow-up logs
each completed bundle build, the whole build pool, and the serial measurement
phase. In the passing full gate, 12 present non-PostCSS bundles built in
1.13–2.20 s each; PostCSS alone took 95.76 s. The complete build pool took
95.76 s and serial warmup/measurement took 2.04 s. The PostCSS graph is open
with five modules, 611 functions and 6,947 sites. Its native-release build is
active C compilation, not a hung process or a slow benchmark workload.

The routine thirdparty performance baseline now selects the 14 baseline names
other than PostCSS (12 present, two absent here) and reports one deferred
bundle. `gate-full` includes all 15 names. Both gates still run thirdparty
correctness parity over every present probe. `thirdparty.sh --update` still
regenerates the full baseline by default; an explicit `--bundles` remains a
targeted selection. On the same source tree and compiler, a cold routine
benchmark built its 12 present bundles in 46.66 s because the shared release
prelude was cold; its immediate warm repeat built them in 1.53 s and measured
them in 1.87 s, with no regressions. The full gate's performance step took
100 s; the warm routine gate's step took 3 s. This is a selection and
cache-state comparison, not a claim that the scheduler itself got faster.

Graph generated-C keys now use a backend module-emitter fingerprint excluding
the separately compiled `src/runtime` C files. The complete backend identity
still includes them. Runtime and prelude materialization retain exact content
digests, and module object keys still include the prelude-header digest. An
existing fingerprint contract test was extended to prove that a runtime-C
change alters the complete fingerprint but leaves the module fingerprint
stable, while an emitter Rust change alters the module fingerprint. The
focused backend crate passed; this cache change should save re-emission after
future runtime-C-only edits, but no wall-time gain from such an edit has yet
been measured.

The final combined `gate-full` passed in 375 s: 2,153 crate tests passed
(nine ignored), native parity 312 matched / one refused / zero diverged,
full Test262 97,183 passing of 102,638 selected with no chunk regressions,
thirdparty parity and all three corpora passed, and both performance and facts
baselines passed. The same-tree routine gate passed in 187 s: crate tests
183 s, native parity 131 s, routine Test262 50 s, and thirdparty performance
baseline 3 s. Its Test262 selection was 101,640 variants with 96,233 passing
and no chunk regressions. The full and routine evidence is under
`post-split-full-gate-final/` and `post-split-routine-gate/` in this worktree.
Both runs used the pinned Clang, a 16-CPU gate budget and hard timeouts;
the watcher confirmed no hangs, storage pressure or orphaned processes.

The remaining routine wall-time target is the cold native fixture lane.
The gate's stable libtest cannot report per-test time: `--report-time` is
nightly-only and was removed after a failed validation attempt. Three
ReleaseSize integration loops are bounded candidates for exact-filter timing,
especially `native_dynamic_import_uses_importer_identity_for_relative_modules`,
but the current logs do not prove which test dominates. For PostCSS itself,
the large release modules already compile under the fast generic `-O0` policy;
the next architectural candidate is splitting a large generated module into
independent C translation units. Per-module source sizes and compiler times
should be measured first, because this run establishes only the bundle-level
95.76 s cost.

## Follow-up: stage thirdparty builds and shorten a native fixture

The non-quick gate now prepares thirdparty benchmark binaries while crate,
native and Test262 correctness lanes run. It reserves one worker from the
shared CPU budget for that build pool. The benchmark worker waits at a
gate-owned release marker after compilation; the gate releases it for serial
warmup and timing only after correctness lanes finish. The marker and worker
are cleaned up on exit, including failure. `gate-quick`, serial gates and
small CPU budgets keep their previous ordering. A fake-kiln barrier test
verified that no benchmark executable starts before release, and the gate
runner's seven focused tests passed.

On the same source tree, the staged `gate-full` passed in 304 s with 2,152
crate tests passed (nine ignored), native parity and thirdparty parity passed,
and full Test262 passed 97,183 of 102,638 selected variants with no chunk
regressions. The full benchmark build pool took 107.54 s and then waited
187.14 s for the gate; serial warmup and timing took about 2.13 s. The staged
routine gate passed in 204 s with the same crate count, native and thirdparty
parity passing, and routine Test262 passing 96,233 of 101,640 selected
variants with no chunk regressions. Its benchmark build pool took 69.59 s,
waited 126.82 s, then measured the 12 present routine bundles; PostCSS was
reported as deferred. All other corpora, the facts baseline and performance
baseline passed in both gates. The full 304 s versus earlier 375 s, and
routine 204 s versus earlier 187 s, reflect changing lane contention and
cache state; they are gate outcomes, not isolated scheduler speedups. The
`thirdparty baseline` step's 301 s / 201 s elapsed figure includes its
intentional wait, so it is not additional post-correctness time. Evidence is
under `post-staged-full-gate/` and `post-staged-routine-gate/`.

A ReleaseSize importer-identity test used `node:assert` only to verify builtin
namespace identity, which compiled a 505-line builtin. It now uses the much
smaller `node:timers` while retaining all three profiles and the `A B true`
assertion. In single-job, cold, exact-filter runs with the pinned Clang, its
wall time fell from 84.45 s to 75.64 s (one sample each); the shared native
setup still costs about 75 s. The integrated full and routine gates above
passed with this change. The resource watcher saw no orphaned processes,
memory exhaustion or disk pressure, and reclaimed roughly 639 MiB of
agent-owned temporary test caches after preserving their timing logs.

One remaining build-tail candidate emerged in both staged gates: the first
acorn benchmark compile took 76.91 s in the full gate and 62.95 s in the
same-tree routine gate, while other non-PostCSS bundles were mostly warm.
The provider staticlib Rust compilation was active during this period. That
correlation has not yet established which cache key or packaging step repeats;
the provider path should be traced before optimizing it.

## Follow-up: retain the provider cache across staged gates

The repeated first-bundle compile was a gate scratch-directory mistake.
`gate.sh` gave the thirdparty benchmark process a gate-owned `TMPDIR` so its
temporary binaries would be cleaned up. The child kiln process inherits that
variable, and its reusable Rust number-provider static library is stored
under `env::temp_dir()/kiln-native-number-provider`. Gate cleanup therefore
deleted the provider archive after every run, forcing the first bundle to
rebuild it on the next run. The benchmark now has a dedicated
`KILN_THIRDPARTY_WORK_TMPDIR` for its output directory; the child keeps the
normal runtime `TMPDIR`. A focused barrier test asserts both paths.

The post-fix `gate-full` passed all ten steps in 275 s: 2,152 crate tests
passed (nine ignored), native and thirdparty parity passed, full Test262
passed 97,183 of 102,638 selected variants without chunk regressions, and
the corpora, facts and performance baselines passed. Its first acorn build
took 7.83 s, versus 76.91 s and 62.95 s in the previous consecutive staged
gates. An upstream emitter change landed between those observations, so this
is not an isolated same-code speedup measurement. The causal path is visible
in the filesystem: the gate-owned stage held only benchmark results, while
the provider archive remained under stable `/tmp/kiln-native-number-provider`
and no provider staticlib rebuild ran. The gate's 267 s baseline-step elapsed
includes 159.85 s waiting for correctness, not an additional serial cost.
PostCSS took 78.28 s to rebuild after the emitter change and stayed in the
full section. Evidence is under `post-provider-temp-fix-full-gate/`. The
watcher confirmed the stage was removed, the provider archive remained, no
gate children survived and resources stayed healthy.

## Follow-up: promote measured Unicode property cases to routine coverage

The prior extended list deferred 499 files / 998 variants, but the latest
complete per-case result showed that 441 generated RegExp Unicode
property-escape files (882 variants) each completed in under one second and
all passed. They summed to 583.2 case-seconds, yet the full Test262 lane took
182 s while the concurrent crate lane took 266 s. Moving those files to the
routine section increases its selected variants from 101,640 to 102,522 and
reduces the extended section to 58 files / 116 variants. All variants of a
file move together. The pinned inventory, exact case IDs and full chunk
baseline remain unchanged; `built-ins/RegExp` extended reference passes drop
from 896 to 14, matching the complete full-run outcomes. The focused
partition check verified an exact disjoint 102,522 + 116 = 102,638 split,
and all 12 Test262 corpus tests passed.

The first combined `gate-full` on the merged tree hit its 420 s cap while
workspace crate tests were still progressing: Test262 and the completed
parity/corpus lanes passed, but the gate verdict was incomplete. This was a
cache-cold run after upstream runtime changes; the extended PostCSS build
alone took 147.76 s and the benchmark pool 335.63 s. The timeout cleaned up
all gate children and its staging directory. The immediate warm retry passed
all ten gate steps in 260 s: 2,152 crate tests passed (nine ignored), native
and thirdparty parity passed, full Test262 passed 97,183 of 102,638 selected
variants without chunk regressions, and every corpus and baseline passed.
That full Test262 lane took 164 s; the crate lane took 257 s. The benchmark
build pool was warm at 8.65 s and remained overlapped with correctness.

A separate bounded routine `mise run test262` then passed in about 157 s at
seven workers and a 7 GiB cap: 102,522 selected, 116 explicitly deferred,
97,115 passing and no chunk regressions. This validates the new routine
baseline and shows the promoted workload still fits below the measured crate
lane. It does not establish a new whole-routine-gate wall time, since that
gate was not rerun. Evidence is under `promoted-unicode-full-gate/` (incomplete
cold run), `promoted-unicode-full-gate-warm/` (passing full gate), and
`promoted-unicode-routine-test262/` in this worktree. The resource watcher
confirmed bounded memory, no storage pressure and no leftover processes or
stage directories after each run. The remaining wall-time target is the
serial workspace crate lane, especially the native suite.

## Follow-up: share CLI native-test artifacts

Two CLI native-build tests, one for the no-extensions Node surface and one
for explain facts, used private fixture caches even though neighboring
semantic tests already use `share_native_semantic_cache`.
They now call that existing helper. Their source programs, flags, native builds,
binary executions and assertions are unchanged; only content-addressed common
artifacts such as the prelude can be reused. The exact two-test filter passed
in 64.79 s in a cold agent worktree. This establishes correctness but not an
isolated speedup: prior gate CLI suite timings varied from 20.82 to 132.15 s
with cache state and concurrent native compiler load.

The first integration gate after merging upstream runtime and corpus changes
reported a separate stale `kiln-bench` expectation: the node:net corpus now
discovers 39 cases rather than 36, while its five selected cases all passed.
The expectation and diagnostic were updated; its exact test passed. The next
gate found that upstream callback descriptor APIs in `node:fs` had changed the
PostCSS whole-program facts. `facts-update` regenerated only the PostCSS
baseline entry; both `mise run facts` and the CLI facts-baseline integration
test passed. The first two integration gates retained their full failure logs
and bounded cleanup; neither is counted as a passing gate.

The final warm routine gate passed all ten steps in 295 s: 2,171 workspace
tests passed (nine ignored), native and thirdparty parity passed, routine
Test262 passed 97,115 of 102,522 selected variants with 116 deferred and no
chunk regressions, and all corpora and baselines passed. The crate lane took
291 s; its largest suites were `kiln-native` 178.82 s, `kiln-daemon` 36.96 s,
`kiln-backend-c` 35.11 s and `kiln` CLI 7.37 s. This run confirms the CLI suite
is no longer the wall-time limit, but it does not isolate how much of that
change came from the shared cache versus warmed artifacts. Evidence is under
`oneshot-shared-cache-gate/`, `oneshot-shared-cache-gate-warm/`,
`oneshot-shared-cache-gate-final/`, and `oneshot-shared-cache-gate-fix/` in this
worktree. A watcher confirmed no remaining gate children or temporary stage
directories and no memory or storage pressure.

The next measured target is native fixture construction. The `kiln-native`
test binary contains 21 pairs of TypeScript-builtin and CommonJS-build tests
with separate graph emission, native compilation and linking. Combining each
pair's assertions into one graph could remove one build/link per pair, but
would change what each test isolates and needs a representative pilot first.
A broader gate scheduler that overlaps Cargo test binaries could also shorten
wall time; direct binary launch would need to reproduce Cargo's runtime
environment and working directory, while separate Cargo invocations may
serialize on the shared target lock. Neither architectural gain has been
measured or implemented here.

After that passing gate, main advanced with worker-transport and native
launcher changes. They merged without conflict. The focused two CLI
native-build tests passed again on the merged source tree in 69.00 s under
a 150 s cap, including the newly changed provider staticlib build; the facts
baseline check also passed. The full gate was not repeated for those unrelated
upstream commits, in keeping with this repository's measurement rule.
