# Body-heavy HTTP throughput: stream correctness costs and missing builtin type metadata

Status: ready-for-agent (remaining structured state-field specialization)

Investigation: 2026-10-12, `investigation/http-body-throughput`, production source
`8b738b253985be434423d2a7be6cf91e3462ecc3`. The initial investigation made no production changes; the targeted implementation and its
measured result are recorded below.

## Findings

Two separate changes explain why the current path is different from the early fast HTTP path.
Their existence is established; their individual contributions to the full historical throughput
change have not been established by a commit-by-commit performance bisection.

1. On October 1, `fe26dd8b` wired IncomingMessage into the shared Readable facade, and `b74af8e7`
   changed HTTP body/end delivery from direct EventEmitter events to the receiver's current
   `push` method. This fixes buffering, pause/resume, delayed end, and overrides. The previous
   shortcut was not a complete stream implementation. The native host now calls the same
   TypeScript state machine ordinary streams use. Its cost must be optimized without reverting
   its semantics.
2. On October 4, `21b515629` made the frontend transform TypeScript to Node-22 JavaScript before
   executable HIR lowering. Authored source/type syntax is retained alongside the transformed
   source. Ordinary compilation reattaches resolved source facts via `apply_source_types` in
   `kiln-db`; embedded builtin compilation calls frontend `parse` and IR `compile_module`
   directly, without that attachment. The builtin path used the same direct compilation before
   the transform, when primitive authored annotations still survived executable HIR lowering.
   The October 7 `e9637a7e` caching refactor retained that path.
3. The October 11 fixes `1dd5a994` and `1e861f78` removed unnecessary state creation for unread
   bodies and legacy response dispatch. The first commit records hello-world falling from
   62.7k to 37.9k and recovering to 65.7k. It intentionally skips stream setup only when no one
   reads the body; JSON echo must still create the state and run its machinery. The second
   records echo CPU/request improving from 76 to 67 microseconds, not reaching parity.
4. The routine gate checks HTTP correctness, but its performance lane is third-party bundles.
   It does not execute `scripts/server-baseline.sh`. The committed HTTP baseline is from
   September 5, uses one load process with 16 connections and compares absolute throughput
   to that old record. Its 1 KB echo row had kiln 14,934, Node 12,164 and Bun 13,228 requests/s;
   the four-process comparison now exposes capacities that a single client cannot load. That
   older apparent win is not an equivalent measurement. Its 64 KB echo row already had kiln
   803 against Node 2,301 and Bun 4,939: the all-workload target was not met even then.

## Type metadata reproduction

`probes/type_metadata_probe.rs` compiles the retained authored HIR and the executable HIR that
the old `builtin_program` used before source-fact attachment. The diagnostic still deliberately
compiles raw HIR; it does not exercise the repaired builtin pipeline.
Run it as a temporary `kiln-builtins` example (the crate already has the required dev dependencies):

```sh
mkdir -p crates/semantics/kiln-builtins/examples
cp docs/issues/server-throughput/probes/type_metadata_probe.rs \
  crates/semantics/kiln-builtins/examples/type_metadata_probe.rs
mise exec -- mbx run --release -p kiln-builtins --example type_metadata_probe
rm crates/semantics/kiln-builtins/examples/type_metadata_probe.rs
rmdir crates/semantics/kiln-builtins/examples
```

Observed output:

```text
typed.ts authored: 1 functions, 1 annotated, 0 checker facts
typed.ts raw executable HIR (before source facts): 1 functions, 0 annotated, 0 checker facts
node:stream.ts authored: 96 functions, 64 annotated, 0 checker facts
node:stream.ts raw executable HIR (before source facts): 96 functions, 0 annotated, 0 checker facts
```

The annotation count includes `any`, `void` and composite hints; it is not a count of 64 functions that can become fast entries. The typed sample is `increment(value: number): number`. This reproduction intentionally does
not invoke the project checker, matching the embedded builtin path at the investigation commit. It does not claim that
ordinary user TypeScript compilation loses all types. The value graph still infers facts and
emits guarded shape/PIC fast paths without source annotations; generated `Readable.push` already
has a receiver-shape guard for `_readableState`.

A second limitation remains even after attachment: `SourceTypeAnnotations` contains only coarse
parameter and return hints. `apply_source_types` consumes Parameter/Return roles, not Expression
facts. Composite record types have no field-layout representation there. `ReadableImplementation`
already declares its state fields as numbers, booleans and arrays, but that declaration currently
cannot select a guarded typed state-field layout. Merely adding more source annotations will not
make those field accesses unboxed.

## Controlled measurement

Machine: Linux x86_64, AMD Ryzen 7 5800H, 8 physical / 16 logical cores. Node 26.8.1,
Bun 1.4.2, Deno 2.9.7. Native-release, one kiln worker. No investigation compilations overlapped
the measured loads. The shared workstation is not an exclusive benchmark runner.

`bench/server/compare.sh`, three passes, four Node client processes, 16 connections each,
2 seconds per measured pass, alternating baseline/candidate order. The duration is shorter than
the published October 11 five-second record, so these are new diagnostic measurements, not a
replacement for the release table. All 180 client runs succeeded: 3,266,152 checked responses,
zero transport errors, zero wrong responses.

| workload | kiln baseline | diagnostic bypass | Node | Bun | Deno |
| --- | ---: | ---: | ---: | ---: | ---: |
| hello world | 60,431 | 60,818 | 45,494 | 53,845 | 43,673 |
| small service | 25,822 | 19,490 | 31,674 | 47,020 | 28,745 |
| 1 KB JSON echo | 14,639 | 17,058 | 25,204 | 35,826 | 35,646 |

The diagnostic sets `http.IncomingMessage.prototype.push` to emit data/end directly. It keeps
listener registration/state creation and the ordinary JSON/Buffer/application work. It bypasses
only part of stream handling, and must not ship: it discards pause/buffering/custom stream
semantics and loses the unread-body shortcut. That latter cost hurts the mixed workload.
The hello control uses the identical hello executable in both slots.

Echo's per-pass candidate/baseline ratios were 1.1697, 1.1852 and 1.1652 (median +17.0%).
Mixed was -25.0%; the identical hello control was -0.8% by paired median. The +17% is evidence
that push handling costs throughput, not an estimate of all stream overhead, a type-driven
optimization result, or a safe patch. It does not close the gap to the other runtimes.

## Runtime profile

A separate native-profile build adds `-pg`, and the app registers SIGTERM to close its server and
exit normally, so gprof writes its record. Twenty-second runs compare the app's own GET root
route with its 1 KB echo route, four clients / 16 connections each. The GET driver checks the
app's exact `hello from the app` response, rather than the standalone hello server's different
response. These instrumented runs are used for call counts/sample attribution, not release
throughput claims. Both accepted runs had zero wrong/error responses.

| helper calls per checked request | app GET | JSON echo |
| --- | ---: | ---: |
| own-index/key lookup | 10 | 105 |
| generic own-property read | 8 | 112 |
| method dispatch | 4 | 40 |
| generic get-site | 2 | 40 |
| new object | 3 | 15 |
| closure creation | ~0 | 6 |
| captured-local cell creation | 1 | 8 |
| computed property definition | ~0 | 11 |

Counts include all application/runtime work, not only Readable. Inline slot reads are not counted
as generic helper calls. The recorded stream symbols identify `createReadableState`, `push`,
`resume`, its deferred drain callback and the deferred `_read` callback. State construction alone
performs 11 computed property definitions and creates two arrays for every request that reads a
body. `push` creates its captured `state` cell before branching, even on its null/end path.

The echo flat profile spreads samples across property/index lookup, method dispatch, allocation,
UTF-8 conversion and JSON serialization. Its largest named helper is only 3.68% of sampled self
time; `string_to_utf8_lossy` is 3.14%, JSON string quoting 1.84%. This does not justify calling one
JSON parser function the entire bottleneck. Gprof's recursive call-graph time attribution and
instrumentation overhead limit precise percentages; retain the raw record and corroborate any
optimization with uninstrumented paired measurements.

Local evidence is retained in `.scratch/http-body/`: build logs, kept-C paths, the two accepted
profile directories, `type-metadata.log`, `paired-push-bypass/` metadata/client records,
`paired-summary.json`, `investigation.json` (source/binary/probe digests and environment) and `paired-push-bypass.log`. The invalid first attempt to profile the app
GET with the standalone hello oracle was rejected and replaced; it contributes to no result above.

## Repair sequence and acceptance

1. Route builtin authored contracts through the same source-type attachment authority as ordinary
   modules. Preserve source-map coordinates and include all attached facts in program/cache
   identity. Add a guard test showing typed builtin facts survive the transform. Treat facts as
   guarded hints; TypeScript declarations never prove what a JavaScript caller passes.
2. Add structured receiver/state-field contracts and expression facts to guarded value-graph
   specialization. Default stream state should get one shape/descriptor guard and direct slot
   operations for its numeric/boolean/array fields. Revalidate after observable calls. Preserve
   ordinary fallback for overridden methods, accessors, changed prototypes, proxies and differently
   typed writes. Explicitly typed, inferred and JavaScript callers must all stay correct.
3. Reduce avoidable allocations/dispatch in the same shared TypeScript stream implementation:
   lazy capture cells on branches that create closures, guarded direct calls to known builtin
   targets, and bulk object-literal shape initialization. Prioritize each against the profile and
   report paired results; metadata preservation alone has not yet been shown to improve HTTP.
4. Make HTTP/stream/compiler changes run a dedicated, quiet paired HTTP performance check, with
   controls and all body sizes, after correctness builds have stopped. Do not silently compare
   to an old machine's absolute requests/s or put a noisy load run alongside compilation. Keep
   the routine gate fast and record the required affected-feature performance evidence explicitly.

A candidate must preserve the existing HTTP Readable pause/resume/end/custom-push tests and Node
oracle behavior, pass the HTTP close contract, affected native/Node/server corpora and GC stress,
then the routine gate before landing. Measurement must include hello, mixed, 1/64 KB echo,
chunked streaming and large upload, correct answers, CPU/request and memory; show the effect on
both engines. No speedup obtained by restoring direct event delivery counts as a repair. New
builtin implementations stay in TypeScript with intrinsics under ADR 0005 and the C freeze.

## Implementation and measured result (2026-10-12)

The builtin pipeline now attaches checked authored parameter/return contracts using the ordinary
IR authority, caches them separately from project identity, and includes the source-types mode in
its program cache key. Method signature matching uses the mapped closing brace where the
executable method span begins at its key but the authored signature begins at `(`. Regression
coverage distinguishes neighboring methods, aliases, computed keys and constructors; it also
checks that wrong-type callers, replacement methods and accessors retain the ordinary fallback.
These are hints, not a proof that JavaScript callers obey TypeScript declarations.

The literal-site shape cache now covers the existing 32-property mask limit, including the
11-field Readable state. GC-owned tail slots retain the same descriptor/prototype fallback and
number-layout guard. Readable.push captures a separate state binding only inside branches that
actually schedule another read, so buffered data and EOF do not allocate that cell. The deferred
callback retains the original state even if a listener replaces `_readableState`. Finally, native
EventEmitter listener-table queries use a descriptor copy instead of materializing a shaped
object's dictionary on every read or miss; writes still acquire actual property storage.

The HTTP harness now records server-only process CPU and lifetime peak RSS on Linux. The affected
feature's quiet paired command is documented in AGENTS.md; it stays outside the concurrent gate.

Final native-release measurement: five alternating paired passes, 5 seconds per load, four client
processes with 16 connections each, all six workloads on the investigation machine above. Before
binaries are immutable builds of `8b738b25`; after binaries are `hello-private`/`app-private`.
No build/test compilation overlapped these runs. All 8,736,470 responses were checked, with
zero wrong answers or transport errors across 240 client runs. Ratios pair each pass; separate
throughput medians need not have the same ratio.

| workload | before req/s | after req/s | paired change | before CPU us/request | after CPU us/request | before peak MiB | after peak MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| hello | 66,599 | 66,979 | +1.30% | 11.91 | 11.85 | 32.1 | 32.8 |
| mixed | 25,737 | 26,152 | +1.63% | 35.80 | 35.25 | 25.4 | 31.8 |
| json-echo-1k | 14,554 | 14,793 | +2.06% | 65.40 | 64.33 | 29.3 | 35.2 |
| json-echo-64k | 2,316 | 2,301 | -0.86% | 421.43 | 423.18 | 51.9 | 58.4 |
| chunked-stream | 61,084 | 61,421 | +0.30% | 13.86 | 13.74 | 28.7 | 34.9 |
| large-upload | 3,889 | 3,943 | +1.18% | 245.91 | 242.57 | 97.0 | 106.7 |

The gains are modest and uneven. This does not establish HTTP parity. Peak memory is higher,
particularly with body traffic; these five-second runs are not a long-duration retention study.
An earlier three-pass candidate showed a 3.3% large-JSON throughput loss; the final five-pass
combined patch shows a 0.9% loss and a 0.4% CPU/request increase there, so it is not reported as a
win. An attempted isolation run overlapped test compilation and was explicitly discarded.

Separate three-pass 1 KB echo control measurements on the same machine gave Node 27,389, Bun
37,268 and Deno 37,647 req/s (33.44, 27.58 and 28.53 CPU us/request respectively). Those controls
were measured before the final five-pass pair, not simultaneously. The size of the remaining gap
is clear; the early apparent all-runtime win is not restored by this patch.

Local evidence: `.scratch/http-body/paired-private-final/` retains binary hashes, alternating
order, raw server/client logs, all aggregates and the paired summary. The discarded isolation
has a `DISCARDED.txt` marker. `paired-private-type-facts/` isolates source-fact attachment with
otherwise identical final native sources. Structured receiver/state-field contracts and their
guarded lowering remain the next compiler task from step 2 above; coarse parameter/return facts
alone cannot make those field accesses unboxed.

With the final runtime held constant, the three-pass source-types-on/off pair measured json-echo-64k: -1.51% throughput, json-echo-1k: -0.76% throughput. This is not evidence that reattaching coarse facts speeds up HTTP. That repair restores compiler metadata and enables the next specialization work; it is not credited with the combined patch's small throughput gain.

Verification evidence is retained in `.scratch/http-body/`: `gate-final.log` and
`target/gate-logs/run.jIqvlMF7/` record the first completed run (722 native matches, six existing
refusals, no divergences; 97,745/102,522 routine Test262 passes with no chunk regressions;
17 present third-party bundles match both engines, five bundles absent). Its only test failure
was the expected internal-helper signature snapshot, now reviewed and updated; the focused
snapshot check passes. That pass also reports mixed code snapshots because the correction
occurred during the run, and it is not accepted as a green aggregate. A frozen rerun's full log
is `gate-verified.log`; read its aggregate envelope for the final verdict and workspace totals.
HTTP close, affected collector stress, realm isolation and harness logs are retained alongside it.
The 116 extended Test262 variants are deferred in the routine gate. Linux arm64 follow-up from
the separate pointer-layout task remains pending; this patch's execution evidence is x86_64.

Affected checks passed on the staged implementation: all six Python HTTP harness tests; the
complete HTTP-close contract (`http-close-final.log`); five native probes with collection
stressed on every allocation, with no refusals/divergences/skips (`gc-stress-verified.log`);
and four probes at 1, 2 and 16 realms with stressed collection, no skips or divergences
(`realms-verified.log`, per-realm evidence `realm-evidence/run.2edTeuW8/`). The four realm probes
cover tail literals, the deferred state capture, source-type method fallback and private brands.
The final frozen gate evidence directory is `target/gate-logs/http-throughput-verified-20261012/`.
