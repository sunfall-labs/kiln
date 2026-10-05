# Direct TypeScript Effect suite

Goal: execute every pinned Effect test directly in kiln, prioritizing native, without
bundling sources. Bun supplies the direct-TypeScript performance comparison; the official
Node/Vitest inventory and per-case outcomes establish correctness and runner fidelity.

## Current checkpoint, 2026-10-05

The last verified public main commit is `bc82ca8f`. Its compiler milestone has a
[passing ten-step routine gate](evidence/spread-child-gate-20261005/), with 2,670
crate passes and zero Test262 chunk regressions across 102,522 selected routine
variants; 116 expensive variants were explicitly deferred.

The committed clock candidate makes explicit interpreter wall-clock execution read
actual system time instead of the output-capture host's synthetic clock.
[Original Formatter, Pool and Layer acceptance](evidence/interpreter-wall-clock-20261005/)
is 153/153, including all 39 Layer cases with unchanged deadlines.
[Native Formatter acceptance](evidence/native-formatter-clock-20261005/) is separately
67/67, exactly mapped to the official Node/Vitest oracle. Those focused reports are
not a fresh full 322-file result. A compiler-owned Oxc disposal helper separately
[passes all nine original ManagedRuntime cases in both engines](evidence/oxc-usingctx-20261005/);
its final disposal edge-case follow-up and shared Array callback changes are still
under validation and have not been landed.

The disposal follow-up is now locally committed with
[final 9/9 ManagedRuntime evidence in both engines](../oxc-using-context/evidence/20261005/).
One general constructor default-prototype mismatch remains under repair; it is
recorded rather than hidden by rereading a Proxy's prototype.
[Deadline adapter checks](evidence/harness-deadlines-20261005/) match actual
Vitest 5.0.3 in Node, Bun, interpreter and native for cleanup continuation,
independent hook budgets, synchronous overruns and falsy thrown values.
The [complete original Cluster interpreter replay](evidence/interpreter-cluster-deadlines-20261005/)
now records all 47 cases: 36 pass and 11 fail at original deadlines, with exact
oracle mapping and no missing cases. This uses the older frozen clock binary,
before final Array/disposal changes, and is negative evidence requiring repair.

The clock milestone's next gate was
[interrupted by an environment transition](evidence/clock-gate-interrupted-20261005/).
Its completed workspace, native parity, thirdparty and Node/server/regression lanes
passed, but it has no aggregate verdict, complete Test262, facts or performance
result and does not authorize landing. Current permissions make main's `.git` and
sibling worktrees read-only and GitHub access is unavailable; source and evidence
are preserved in an isolated writable clone pending validation and publication.

Native Sharding's six original five-second timeouts and ClusterWorkflow's one timeout
remain unresolved. Complete final-source 322-file native and interpreter acceptance,
the required gate, and a quiet 22-microbenchmark/four-real-world direct-TypeScript
comparison against Bun are still required before completion.

## Earlier checkpoints, 2026-10-04

The latest integrated module-generation and top-level-await candidate passes 255 interpreter tests
(one ignored), 29 module-evaluation tests and all 195 unchanged cases in `Config`,
`EffectKeepAlive`, `StackTraceLimit`, `rpc/RpcClient` and `schema/HMR`.
[The validation report and raw logs](interpreter-module-generations/evidence/reset-tla/)
bind the source, frozen CLI, pinned test sources and official oracle. This is targeted
interpreter evidence. The resumption fix preserves a suspended module's activation and
generation, waits for its evaluated namespace, and retries dependency scheduling when any
pending dependency settles. Regression checks cover reset during an externally suspended
await, dependency wake ordering and private engine bindings across suspension and rejection.

The [earlier complete gate attempt](milestone-gate-full-20261004/) passed 2,615 crate tests,
third-party correctness and all 13 unchanged third-party performance baselines, including
PostCSS, after external task workers were stopped for timing. It failed native parity on
three parallel-allocation probes and aborted one full-Test262 batch at its 1 GiB process
cap. The [gate memory-floor repair](../gate-test262-memory-floor/evidence/) reruns that
exact batch at 4 GiB without an allocation abort; it does not replace a fresh full gate.
Native allocation diagnostics identified omitted metadata in duplicated parallel object
and array initializers, plus an uninitialized generation flag in worker nursery chunks.
Worker chunks bypassed the ordinary nursery reset: a stale `tenured` byte made minor
collection skip reachable arrays, then reclaim them during the flattened-result allocation.
The [shared chunk-header repair and evidence](../parallel-builtins/evidence/parallel-chunk-tenuring-20261004.md)
include a poisoned-memory child-survival regression that fails against the exact original
initializer and passes after the repair, with an assertion that collection was minor.
All three original parallel-allocation probes now match. The expanded small GC-stress
companion also matches, retaining nested flatMap children across a sequential allocation
replay. These targeted checks do not replace final-candidate native stress.

The subsequent full gate (`run.VO6m36gX`) completed with full Test262 coverage:
102,638 selected variants, zero deferred, and no baseline regression across 122 chunks.
It failed overall after disk exhaustion disrupted crate, native, third-party and regression
lanes. Its one identified source-level snapshot failure is now repaired and passes its
focused test. [The aggregate and disk recovery record](resource-coordination-20261004/post-gate/)
retain that failed integration result. A quiet five-bundle performance replay passes the
unchanged thresholds; this does not replace a fresh passing full gate.

The [complete 650-probe native allocation-stress run](allocator-full-stress-20261004/)
records 580 matches, five named refusals, 45 declared stress skips and 20 HTTP divergences.
Those HTTP failures exposed a full-GUI provider package built without `realm-http` despite
advertising HTTP capability. The feature contract is repaired with a red/green regression,
and the [corrected package's focused checks](allocator-full-stress-20261004/http-provider-fix-focused/)
match all 20 HTTP probes and three GUI probes under allocation stress, with one declared
GUI stress skip. The historical failed run remains retained. The [fresh full integration gate](allocator-gate-green-20261004/)
(`run.zqPsS00h`) passed all ten steps in 1,307 seconds with an eight-CPU budget and a
4 GiB Test262 cap: 2,635 crate tests passed with zero failures, native parity matched
645 probes with five named refusals and zero divergences, and all 102,638 Test262
variants were selected with no chunk baseline regression. All 17 present third-party
bundles matched and all 13 present unchanged performance baselines passed, including
PostCSS. The candidate CLI is frozen as
`16958abcd29f328d23bb789fc188f0e55a065e349b0b77736ce0354ba1820ceb`.
The [complete corrected-provider allocation-stress run](allocator-full-stress-corrected-20261004/)
exited 0: 600 matched, five named refusals, 45 declared stress skips and zero divergences.
All 650 entries are accounted for, using the same frozen CLI and unchanged probe runner.

The runner's opt-in artifact compression and shared-cache collection have also been
[validated against 43 unchanged Boolean cases in native and Bun](runner-resource-policy-validation/).
Archives are verified before deleting raw binaries; cache sweeps address the same first
writable layer as native builds. Resource recovery disabled mbx's automatic shared-target
pruning through its supported configuration interface after an automatic pruning incident.

The stopped older native survey covers 257 of 322 files; 65 remain unmeasured. It is not
final-candidate coverage. [Resource coordination evidence](resource-coordination-20261004/)
records the preserved survey and verified binary archives. Full 322-file interpreter and
native validation and final quiet Bun benchmark comparison remain required before
completion. Full integration and corrected native allocation-stress validation are green;
the validated compiler milestone was pushed to main as `2b83b0b5` (remote hash verified).
The fresh serial run selects all 322 files, native first followed by Bun and the interpreter,
with no deferred files and unchanged upstream case deadlines. Its first file, `Array`,
records 146 passes in each engine, with complete official-oracle mappings for kiln.
That early result is not a full-suite score.

A [quiet replay and attributable native profile](sharding-current-symbolized-build-20261004/)
confirm all 66 current Sharding cases were recorded: 60 pass and the same six fail at their
original 5-second deadlines. The profile retains the exact ELF Build ID and local symbols;
CPU hotspots are evidence to investigate, not yet a diagnosis or optimization result.

The [selected async-reset diagnostic](sharding-async-reset-native-diagnostic-20261005/)
compares unchanged test bodies with their original five-second deadlines. The native
remote-reset control passes after ten storage reads; the async-reset case times out after
26 reads, while pristine and instrumented Bun controls both pass and the instrumented Bun
case completes 127 reads. Each native case sends one persisted local request with no outgoing
retry churn. This narrows the investigation to progress during the storage-reset path; it
does not yet distinguish execution cost from scheduling behavior or establish a fix.

## Corpus and measurement

`conformance/effect-upstream/corpus.json` pins Effect 4.0.0 at
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`, including all 322 test files.
The official Node/Vitest run registers 11,019 cases: 10,974 pass and 45 skip.
Evidence: `.scratch/effect-oracle/all.json` in the owned effect-complete worktree.
The [Porffor page](https://porffor-effect.porch.page/) uses Effect's own
`effect@4.0.0` test files and links back to that upstream tag. Its October 2 run
lists 322 files and 3,454/8,592 passing cases (40.2%), using Porffor `72d048d7`
and Node 26.3.0. The published metadata says `bundled: false`. The page counts
only cases passing under its Node runner, excludes unavailable packages/files
that fail under Node, and polyfills TextEncoder/TextDecoder. Its denominator
and adapter therefore differ from our complete official Vitest inventory;
the percentages are not matched coverage. `porffor-reference.json` retains
the retrieved metadata and evidence digests.

The complete Bun 1.4.2 direct-source reference registers the same 11,019 cases:
10,936 pass, 28 fail and 55 skip across all 322 files, with no incomplete files.
Ten failures name unimplemented loader/reset capabilities; the remaining failures
retain genuine runtime, formatting, assertion or child-process differences from
the official Node run. `bun-reference.json` records the case IDs, pin, adapter
digests and full report digest. These are correctness counts, not performance
measurements. Runtime-specific skip modes are preserved and compared explicitly.

The runner keeps Bun as the direct-source reference for ordinary cross-engine comparisons,
but runtime-conditional cases use exact per-engine expectations in
`conformance/effect-upstream/engine-mode-policy.json`. Ten GC cases across Cache,
MutableHashMap, Pool, ScopedCache, McpServer and Atom have an upstream guard on
`process.versions.bun` or `process.versions.deno`. Bun skips these; Node, the interpreter
and native run them. The policy records each exact test ID, source-file digest, guard,
expected modes and outcomes, and the official Node/Vitest pass evidence and report digest.
Only listed cases receive this per-engine comparison; an unexpected mode or outcome remains
a mismatch, with mode and outcome mismatches reported independently. A Bun
`HARNESS_UNSUPPORTED` result is a separate adapter fidelity gap: the policy does not turn a
native pass into a Bun failure or skip, and the runner continues to report that direct-Bun
divergence until the harness capability has an appropriate oracle.

The complete Node/Vitest results are now a separate, archived oracle rather than only a summary.
`conformance/effect-upstream/node-vitest-reference.json` stores its per-case pass/skip outcome,
the Node 26.8.1 and Vitest 5.0.3 provenance, the official report SHA-256, and every pinned test source digest. Its counts match the direct Bun inventory: 11,019 cases across 322 files. Unique
exact case IDs map automatically (10,713 cases). All 306 parameterized or duplicate
labels now have explicit one-to-one mappings in `node-vitest-case-mapping.json`,
with pinned source digest, source line, parameter-table values or duplicate occurrence
evidence. Nested parameter tables retain their full index path; no ordinal fallback is used. Reports expose
native/interpreter conformance to Node/Vitest separately from direct Bun differential results,
and keep unmapped direct cases, unmatched official cases and adapter `HARNESS_UNSUPPORTED`
results visible. These gaps prevent an incomplete mapping from being reported as complete
conformance. The completed exceptions cover 278 expanded parameterized labels in 24
files, 14 repeated-label occurrence IDs in 7 files, and 14 duplicate official titles in 7 files.
All 11,019 cases map one-to-one in the complete synthetic inventory check. This is
runner identity coverage, not proof that every runtime case has passed.

`node-vitest-comparison-gaps.json` retains known runner transformation differences by exact case
and source digest. In particular, Vitest transforms TypeScript before the Formatter function
string assertion, while direct-source execution preserves original source. The report retains
the official outcome and the actual direct-source outcome as a named mismatch; it never counts
that case as a pass by normalization.

The earlier authored-source path failed `Formatter > format > function`: its assertion expects
 tab indentation and a trailing semicolon, whereas the pinned TypeScript spells spaces and no
semicolon. The production per-module TypeScript transform now makes emitted JavaScript the
parser/span buffer and retains original TypeScript plus mappings for diagnostics. A fresh
unchanged-input interpreter replay on pushed `e8c82292` passes all 67 Formatter cases, exactly
mapped to the official Node/Vitest oracle. Older Bun/native failures remain historical evidence;
a fresh native Formatter replay is still required. The same compiler passes all 47 interpreter
Pool cases, including the four GC-retention cases; Layer completes 38/39 with one unchanged
five-second timeout. Complete reports and logs are in
[evidence/formatter-pool-layer-20261005](evidence/formatter-pool-layer-20261005/README.md).

The latest complete selection on native compiler `8cb04e4f` recorded 10,785 passes,
142 failures and 45 skips. Two files stopped early: 47 registered cases were not
recorded, so this is an incomplete diagnostic run, not a complete native score.
The same run reproduced Bun's complete reference exactly. The earlier compiler
`e445e822` recorded 9,379 passes, 796 failures and 799 missing results.
`native-diagnostic-reference.json` retains compiler and report digests, source
and adapter digests, per-file completeness and failure IDs. The midway compiler
predates subsequent fixes to generated modules, dynamic classes, UTF-16 string
replacement, text encoding and WebCrypto. Focused results from those fixes must
not be added to this older run. A fresh complete run on one frozen compiler is
required for an updated comparison.

A later complete 322-file selection on frozen binary `cff955384200` recorded 10,815
native passes, 21 failures and 45 skips, with two incomplete files (Cause and
SchemaAOTCompiler). It ran the development profile and predates the subsequent GC,
module, AbortSignal, process and production-profile fixes. Cause encountered workspace
interference; the AOT build exceeded its outer deadline across a documented gate pause.
Neither is a passing result. The complete Bun reference again reproduced 10,936 passes,
28 failures and 55 skips. Raw evidence remains in
`.scratch/native-full-completion-current/report.json`; focused fixes are not aggregated
into this diagnostic. A fresh isolated production-profile run is required.

A separate frozen run on `e61fe6fb` passes all 46 TestSchema cases after sharing
strict graph comparison with `node:util` and fixing native own-property
enumerability. Its three-file selection still records two native RegExp codec
failures and one native property-based diff/patch failure; Bun instead fails
the File/FormData codec cases and the property-based case. The exact IDs and
report digest are retained in `native-focused-reference.json`.

Later focused runs on frozen compilers pass all 115 selected compression and
SchemaAST cases, and all 206 selected Crypto and SchemaBinary cases, under both
native and Bun. Generated-schema tests pass 14 of 15 under both runtimes; native
still misses the linked child runner's completion output, while Bun's child fails
in its internal stream initialization. Matching outcome counts do not resolve
those different failures. `native-focused-reference.json` retains the separate
compiler identities, source/adapter/report digests and failing case evidence.
These subsets are not an updated whole-suite score.

Subsequent frozen diagnostics pass all 223 codec cases and all 46 TestSchema cases on native.
The linked-source URL fix passes all 11 SchemaAOTCompiler cases. Native Pool passes all 47 cases,
including four GC cases that Bun skips; interpreter Pool still fails those four GC cases.
With the production `native-release` profile, JSON patch passes all 10 cases on native while Bun
passes nine. Production Arbitrary initially recorded 176/178 native passes during overlapping gate work.
Execution replays of the same binary now pass all 178 cases, matching Bun, with the original
five-second case budgets. The quiet replay took 20.6 seconds without an outer memory cap; a
second replay with the matched 8 GiB cap also passed all 178. The focused evidence retains these
separate conditions, compiler digests, inventories and profiles.
These results are separate selections and do not replace the incomplete whole-suite diagnostic.
A later URL-focused production run passes all 19 unchanged cases under native and Bun.

The next milestone's first gate attempt is not green: routine Test262 passed its chunk floors
with 97,606 passes, but the benchmark preparation lane timed out waiting for correctness lanes
under a six-worker budget. The run was stopped and its full evidence retained. A complete gate
must be rerun on the final integrated fixes before that milestone is pushed.

The subsequent integrated gate (`run.vugC5khE`) passed native parity (635 matches, six named
refusals, zero divergences), routine Test262 (97,606 passes), third-party correctness, the Node,
server and regression corpora, and the 31-target facts baseline. It failed eight crate tests
(one missing private-slot reservation, one obsolete launcher-text assertion, and six native
top-level-await lifecycle cases), plus the benchmark coordination timeout during cold builds.
Those failures remain recorded; the slot, assertion and TLA fixes have focused verification,
and the subsequent final gate passed before pushing. Full allocation-stress parity on the
integrated compiler separately passed 591 matches, five refusals and zero divergences, with
45 explicit stress skips. Stress execution paused only its dispatcher during the gate; active
programs were never suspended.
The six TLA failures now pass within 14 focused native tests. A separate production
build failure in Cluster and Sharding came from a regex pattern in a generated C
comment: C trigraph translation joined the next declaration into that comment.
Generated regex comments now contain fixed text; a matching regression passes
normal and allocation-stress parity. The resulting production Cluster run records
all 47 unchanged cases: 46 pass and one retains its original five-second timeout
(parent suspended past the activity interrupt retry budget). Sharding is being rerun.
Shared TypeScript URL state is now non-enumerable, and strict equality compares URL
values by href plus enumerable user properties; three focused parity probes pass
normally and four pass with allocation stress. The native parity runner now compares
captured raw files, preserving NUL bytes and trailing newlines. Persistent controls
exercise byte differences, equal binary output and exit-status-only differences.

The final integrated routine gate (`run.TGCbJX4K`, frozen `e872e3a0`) passed all ten
steps in 1,227 seconds: 2,565 crate tests passed with zero failures (110 suites,
nine ignored); native parity matched 637 probes with six named refusals and zero
byte/status divergences; routine Test262 passed 97,606 of 102,522 selected variants
without a regression across 122 chunks (116 extended variants deferred). All 17
present third-party bundles matched, all 12 routine performance baselines passed,
and the Node/server/regression corpora and 31-target facts baseline passed.
Pre-push HTTP and native-provider contracts and core crate tests also passed.
This milestone is on main at `e872e3a0`; `gate-green-reference.json` retains the
frozen source/binary/report digests. Complete production Effect selection and
remaining Cluster/Sharding timeouts remain outstanding. A subsequent quiet Sharding replay
using the identical production binary passes 60/66 cases: six original five-second
timeouts remain, while the max-resident-entities case passes without competing builds.

The [complete 22-row microbenchmark comparison](../../performance/microbench-2026-10-04.md)
reports no checksum divergences. Native beats Bun on six rows and is within 10% on eight;
object allocation and Buffer concatenation remain major performance debts. These JavaScript
microbenchmarks are separate from the direct-TypeScript Effect inventory. A subsequent
[Buffer.concat optimization](../../performance/buffer-concat-2026-10-04.md) reduces its quiet
warm native time from 1,162 ms to 28 ms (41.5×), with matching checksums; Bun takes 6.125 ms.
The integrated Buffer milestone passes all ten routine gate steps; `buffer-gate-reference.json`
retains its frozen evidence.

The later production diagnostic exposed 84 Cron and DateTime failures. The specialized
`arguments.length` emitter updated its boxed value but left live numeric representations stale,
causing Date setters to lose optional arguments. The fix preserves those representations:
all 43 Cron cases and 94 runnable DateTime cases now pass with one upstream skip, using unchanged
pinned sources, production optimization and a frozen provider. `date-cron-reference.json`
records the focused evidence. These results do not replace a complete 322-file production run.

Native HttpClient now passes all eight unchanged upstream cases against Bun, including
interrupting an unbounded response and closing a body reader early. Fetch publishes headers
and streams the body instead of waiting for the entire response; cancellation closes the
transport, and the server honors its half-close policy. The native regression matches Node
under ordinary and allocation-stressed execution, and the HTTP close contract passes.
`http-client-reference.json` records this focused evidence; 321 files were deferred.

The streaming candidate is not yet a landed milestone. Its integrated gate was cancelled after
three existing raw HTTP request-FIN tests hung; routine Test262 had already passed its unchanged
97,606-pass chunk floors. The FIN fix must preserve synchronous response flushing, default
cancellation of an unfinished response, and explicitly enabled half-open behavior. A separate
interpreter defect made fetch cancellation reject the reader without closing its client socket:
the private native cancellation hook had no interpreter equivalent. The added interpreter hook
passes the original live-response cancellation fixture and its graph regression, including abort,
reader cancellation and cancellation before connection assignment. Native FIN and cancellation now match Node in two focused native probes under ordinary
and allocation-stressed execution. Nine module-graph HTTP tests and 317 backend-C tests
pass (one backend test ignored). The [final integrated HttpClient replay](http-client-fin-reference.json)
passes all eight unchanged upstream cases under native-release with the frozen compiler/provider.
The integration gate retained 2,574 passing crate tests and found no routine Test262 chunk
regressions across 102,522 selected variants. It failed three ownership/snapshot checks
(now corrected), one interpreter redirect-error divergence, four GUI probes from an incorrectly
pinned headless provider, and the RxJS performance baseline. All four GUI probes pass with
the source-matched full-gui provider. Redirect and performance investigation remain open;
this gate is not a passing milestone. A fresh complete 322-file native-release/Bun replay
is running on the frozen integration compiler/provider as diagnostic evidence.

The object initializer now explicitly clears `external_owner`, including reused arena storage;
a poisoned-memory fixture reproduces the previous uninitialized field. The inline-object
finalizer also skips cleanup when no separately owned side storage exists. Its backend object
suite passes 82 tests, and four affected allocation-stress probes match. A method-name index
passes its 83-test object suite plus representative multi-realm probes with and without GC
stress, but has not increased the quiet Sharding result beyond 60/66. These candidates need the
integrated gate; diagnostic profiles under concurrent builds are not accepted timing gains.
A subsequent [quiet paired allocation comparison](../../performance/object-allocation-2026-10-04.md)
reduces native warm time from 5.0625 ms to 4.4375 ms (12.3%), with matching checksums;
Bun takes 0.0188 ms. The pair includes both object initialization and finalizer changes.
Benchmark result validation now checks every independent checksum sample, with five controls
passing, so a later repetition cannot silently report different work as a timing result.

Wall timers now arm, refresh and rearm from the current call time, including after synchronous
work; provider tests and native parity cover this correction. Release builds can retain symbols
with `KILN_NATIVE_KEEP_SYMBOLS=1` for profiling without changing compilation optimization.
The diagnostic option has a separate release link cache identity.

`scripts/effect-upstream.py` prepares an owned, locked cache workspace and loads
TypeScript test sources directly. It pins adapter dependencies, imports upstream
`vitest.setup.ts`, retains per-file logs and case IDs, and records timeouts,
missing inventories and missing summaries as incomplete runs. Bun and kiln use
the same source graph, setup and adapter. No bundler participates. Native defaults
to `native-dev`; `--native-profile native-release` selects production optimization.
The report records the selected profile, and upstream case timeouts remain unchanged.
Native runs now snapshot the provider manifest and archive as well as the compiler, verify
the archive digest, and pass that snapshot to builds and child launches. `--provider-manifest`
selects a package explicitly; otherwise the runner uses an installed package or builds one
from this repository through mise. Reports include source/archive/manifest digests, ABI,
Rust identity and target. The previous production batch did not capture the provider this way
and remains diagnostic evidence. Ten snapshot controls and three actual upstream files
pass on native (28 cases); Bun passes 27 and fails its AOT integration case while lazily
initializing stream primordials after the fixture replaces Function.
Neither correctness run is a steady-state performance benchmark.

The adapter uses actual Vitest assertion/spy libraries, but does not yet implement
all Vitest loader capabilities. `HARNESS_UNSUPPORTED` is unresolved work, never a
passing test or an upstream skip. Official Vitest passes all runnable cases.

## Implementation direction

TypeScript erasure and enum lowering belong in the common frontend/Owned HIR.
Native DCE must erase expression-only TS wrappers before JS-oriented Oxc folding;
it must not introduce an independent runtime TS transform. Optional chains,
reference evaluation order and method receivers must retain their semantics.

Indirect native eval must dispatch to the existing dynamic evaluator, preserve
global versus lexical scope, and reject unsupported source catchably. Syntax
validation must precede source execution.

Vitest module reset requires distinct source and module-instance identities.
Fresh instances stay in the same realm (including `Symbol.for` identities), and
old closures retain their instance's imports and cells. Native immutable code can
be shared; mutable link state must belong to a rooted module instance. The realm
owns shared host services. A global active-generation switch would incorrectly
redirect old closures. Mocking needs an explicit test-loader capability and import
facades; ordinary ESM namespace objects retain their language semantics.

## Outstanding

- Complete native direct-source execution and compare every case against Bun.
- Match official runner setup, lifecycle, mocking and module resets.
- Implement actual GC-related capabilities where required; do not fake `gc`.
- Address divergences by semantic owner and retain focused native parity probes.
- Run the repository gate before each finished milestone is pushed to main.

## Interpreter collector integration

The integration at `d7677cce` shares suspension-local liveness between the native
emitter and interpreter, traces promise sidecar/context owners, and implements
WeakMap ephemeron activation. Keys are weak; a reachable map retains its value
only when the key is independently reachable. Activation iterates to a fixed point
for chained maps. Unknown owners remain conservative roots.

The integrated interpreter and graph checks pass 275 tests (one ignored), and
three affected native suspension-liveness checks pass.
`collector-integration-reference.json` records source and log digests. The owned
upstream Pool retest still passes 44/47: retention assertions at source lines
871, 935 and 966 remain unresolved. These focused checks are not full Effect or
native GC-stress verification.

## Real-world benchmark correctness

A four-program run on the pushed `340a7d88` compiler passes CLI checksums for
all three repetitions on native, the interpreter, Bun and Node. Native builds
use `native-release`, including the filesystem program. The server client now
shares one keep-alive agent: a regression test verifies 100 responses arrive
over one connection, and the benchmark verifies 2,000 correct responses per
runtime. The previous per-request agent allocation measured repeated
connections and must not be described as keep-alive throughput.

`realworld-main-diagnostic-reference.json` retains the binary and runner
identities, every CLI checksum digest, server response counts and raw report
digest. Execution was pinned to CPU 14 while other jobs continued; timings are
bounded-background diagnostics rather than accepted final performance claims.
CLI process startup is included and native AOT build time is excluded.

The ongoing frozen production survey passes native Pool 47/47, including all
four GC cases skipped by Bun. `pool-native-reference.json` retains the exact
compiler, provider, source and adapter identities plus all case records. This
uses frozen compiler `57424396`, not the later unlanded prepared-call compiler,
and does not add focused successes to an older whole-suite score. The
interpreter retention failures described above remain unresolved.

## Allocator milestone recovery

The frozen combined compiler `f1c8f22d` completed full Test262 coverage (102,638 selected variants, no deferred variants, no chunk regressions), but its full gate failed after the disk filled. All seven reported native divergences were infrastructure failures. The four GUI build failures separately required a provider carrying GUI capability.

The reviewed prelude snapshot update passes its focused test. A full-GUI package was built and verified, and all 14 affected native probes now match. A quiet five-bundle performance recheck passes the unchanged thresholds, including the three rows flagged by the failed gate. [Recovery evidence](allocator-gate-recovery-20261004/README.md) retains the exact logs, provider receipt and latest Pool result: interpreter 44 passed / 3 failed / 0 skipped out of 47.

Full native GC-stress validation is running in an isolated worktree from `3d41ebb2`, using that frozen compiler and the complete 650-file native probe inventory. This is not yet a passing stress milestone or a replacement full gate. The entire 322-file upstream candidate replay, final quiet Bun comparisons, remaining retention and timeout failures, and main integration remain outstanding.
