# HTML rendering: repeated flat-string copies and replacement allocation

Status: ready-for-agent
Completed: private loop accumulation; remaining work: replacement allocation
Measured: 2026-10-12
Investigation tree: `e03aaf36` (`investigation/html-render`)
Reference compiler: published kiln v0.1.0 at `5814f421d547dce3ff4e3f9e1ab05373558b889f`

The website result is reproducible. This is an engine performance problem on ordinary string
construction, not a wrong checksum, startup anomaly, or an accidental debug build. The private
loop-accumulator optimization is now implemented; replacement allocation remains follow-up work.
The published benchmark and its source remain unchanged.

## Reproduction and measured isolation

The fixture creates 2,000 products, escapes each SKU and title through four global regex
replacements, appends each HTML row to one growing string, and hashes the complete document.
Correct output: `rows=2000 bytes=186649 fnv=44702bb6`, with the same leading/trailing slices.

A private worktree, frozen published compiler, `native-release`, Linux x86_64 Ryzen 7 5800H,
logical CPU 15, nine shuffled process runs per variant. All compiles finished before the final
measurement. Every run's complete stdout must equal Node's answer. CLI medians include launch,
startup and work, exclude compilation. These source changes are diagnostic ablations, not a
replacement for the cross-runtime benchmark.

| Variant | Median wall ms | Min–max ms |
| --- | ---: | ---: |
| Original kiln v0.1.0 | 95.16 | 94.78–98.35 |
| Same program, compiler built from main e03aaf36 | 93.51 | 92.81–93.77 |
| Collect rows, `join("")` once | 64.41 | 64.00–65.88 |
| Hoist the four regex literals | 89.08 | 87.89–92.35 |
| Single-pass escaping, original appends | 67.23 | 66.78–69.91 |
| Single-pass escaping and join | 25.99 | 25.63–26.65 |
| Literal-string `replaceAll`, original appends | 84.81 | 84.23–87.41 |
| Literal-string `replaceAll` and join | 45.41 | 44.78–45.94 |
| Perry, original fixture/executable | 23.06 | 21.86–165.91 |

All same-size variants produce byte-for-byte identical output, SHA-256
`4e8425f6a32f5f86524311ce0ceeac31ab61ba06615b531969b5ee77f7ec4571`.
The Perry run has one large outlier; retain it rather than silently remove it. This investigation's
Perry median does not replace the website's earlier seven-run 20.90 ms median.

Phase instrumentation of the original: product setup 1–2 ms, HTML construction 78–80 ms,
checksum 5–6 ms across five runs. The checksum and console output cannot explain the gap.

## Cause 1: flat concatenation repeatedly copies the entire prefix

`crates/native/kiln-backend-c/src/runtime/text.c`, `kiln_c99_concat`, allocates an exact-size
flat result then copies both operands. `html += row` therefore copies every previous row again.
Baseline main e03aaf36 retains this behavior. Repeated appends have quadratic copy
volume; a compiler or representation optimization is needed to make this idiom scale.

A private `LD_PRELOAD` interposer counted dynamically dispatched libc `memcpy` calls and requested
bytes, independently of timing. Inline copies are not counted, so these are observed traffic,
not a complete memory-bandwidth measurement. Timing samples do not use this interposer.

| Executable | Observed memcpy calls | Observed requested bytes |
| --- | ---: | ---: |
| Original, 1,000 rows | 219,828 | 47,253,921 |
| Original, 2,000 rows | 443,828 | 187,921,727 |
| Original, 4,000 rows | 891,828 | 752,676,899 |
| Current main, 2,000 rows | 443,828 | 187,921,447 |
| Join variant, 2,000 rows | 441,828 | 2,465,944 |
| Manual escape plus join | 170,268 | 2,109,129 |
| Perry original | 344,232 | 1,874,317 |

Doubling rows approximately quadruples copied bytes. The output is only 186,649 bytes; original
kiln copies roughly 1,000 times that volume. The join variant eliminates about 185 MB of observed
copy requests. Its wall time improves by about 31 ms. Original timings at 1,000/2,000/4,000 rows
are 35.97/95.16/310.42 ms, with each size checked against its own Node answer.

Perry's observed traffic demonstrates that it avoids these large repeated prefix copies in this
program; this investigation does not infer its exact internal string representation.

## Cause 2: four replacements per field take a general, allocation-heavy path

There are 16,000 `replace` calls per document. The shared TypeScript wrapper in
`crates/semantics/kiln-builtins/src/regexp.ts` performs protocol lookup and `Reflect.apply` with
fresh argument arrays. The native `kiln_c99_string_pattern` path in `runtime/strings.c` converts
subjects/replacements to temporary UTF-16 arrays and builds results through additional temporary
strings. These costs matter even for tiny ASCII strings and single-character patterns.

Hoisting literals only saves about 6 ms; it does not remove the replacement work. The generated
literal path uses `kiln_c99_new_regexp_matcher` with compiled matchers, so blaming recompilation
of regex source on every call would be misleading.

A native-profile build of the untouched source was executed 50 times and merged with gprof.
Its call graph locates heavy work under the escape function, string replacement/protocol dispatch,
and GC. It records about 32,092 `new_array_with_capacity` and 76,433 `gc_alloc_units` calls per
execution. Mid-turn collection accounts for about 26% of attributed profile time. These counts
include bootstrap and instrumentation; gprof changes inlining and can perturb allocation and
GC. They locate costs rather than establish exact percentages of the release executable's wall time.

The output-preserving single-pass escape loop plus join reaches 25.99 ms, 3.66× lower time than
original kiln. The ablations are not additive: changing allocation affects collection scheduling.
The remaining time includes numeric formatting, row construction, hashing and startup. The profile
also shows `toFixed`/exact-decimal conversion work; it is a secondary target, not the main cause.

## Optimization coverage before this change

The runtime already has efficient building blocks: `kiln_c99_join_all_strings` sums lengths,
allocates once and copies each piece once; `kiln_c99_text_append_grow` maintains a growable private
text buffer. Before this change, neither was an automatic lowering for a JavaScript string accumulator. Ordinary
string addition reached `kiln_c99_concat`. The Value Graph escape pass's scalar-replacement
candidates are `NewObject` values; it does not recognize a loop-carried string and replace it with
a builder. This was a missing optimization, rather than an existing accumulator pass whose guard
failed on this fixture. A UTF-16-safe accumulator can reuse appropriate representation mechanics;
the existing byte-oriented text builder alone is not proof of lossless JavaScript string behavior.

## Implementation direction and acceptance

1. Optimize accumulating strings in the Value Graph or string representation, with an amortized
   builder/rope or equivalent guarded strategy. Do not mutate a string whose previous value is
   observable through an alias or captured closure. Preserve append/coercion/exception ordering,
   UTF-16 units, astral text, lone surrogates, and actor/shared graph boundaries. Automatic
   optimization of the original idiom is the target; changing users' programs to join is a workaround.
2. Reduce temporary allocations in the shared replacement/protocol implementation and its lowering.
   Preserve custom `Symbol.replace`, overridden methods/exec, lastIndex semantics, callbacks,
   capture expansion (`$&`, `$1`, etc.), and observable coercion order. Keep built-in semantic work
   in TypeScript with intrinsics per the C runtime freeze; do not add a benchmark-specific C built-in.
3. Keep this original checksum fixture and a growing-size workload as performance evidence. Require
   Node/interpreter/native parity, relevant string/RegExp Test262 subtrees, alias/coercion probes,
   `native-stress` after allocation/rooting changes and `native-realms` after shared cache changes.
   Run the repository gate once the implementation is finished, before landing it.

## Reproduce

`template-render.ts` is the immutable fixture. `reproduce.py` generates all ablations, builds them,
checks complete Node output, then measures in shuffled order after compilation is finished:

```sh
python3 docs/issues/html-render-performance/reproduce.py \
  --kiln /path/to/published/kiln-0.1.0/bin/kiln \
  --work .scratch/html --cpu 15 --passes 9 \
  --perry-binary /path/to/original/perry-template-render \
  --current-binary /path/to/current-main-original-render
```

Omit peer/current binaries if unavailable. Add `--optimized-binary /path/to/optimized-original-render`
to include an unchanged 2,000-row source compiled by the candidate in the same shuffled run.
Build the 1,000/4,000-row variants with both compilers to repeat the scaling comparison.
Choose an allowed CPU on a different machine.
The private work directory holds generated sources, full build logs and `results.json` with every
sample and source/output/compiler hashes. The measured current-main executable SHA-256 is
`69f3c1541f8b092b83511e9a59aeab1fe3474bc549e8789fdd6b073fc6c76c01`;
its compiler SHA-256 is `59e58a1ebab54d21925669d0cd9726da96aea0ce4f981256a7637a878df4f37e`.

Local evidence for this investigation: `.scratch/html/results.json`, `final-run.log`,
`copy-counts.json`, `copy-count.c` / `.so`, `profile.txt`, `gmon/`, and the retained native-profile
build directory `/var/tmp/candrewlee/kiln-native-1795832-0-1791841579055473298`.
Raw timings and profiles remain local; the figures above are the durable review record.


## Implemented: private loop-carried accumulation

The native Value Graph recognizes a string-literal-initialized loop phi whose only in-loop uses
are its append and the associated private local store. Captured cells, parameters, direct eval,
suspension, observable script globals and retained intermediate values are excluded. Conditional
updates whose back edges merge through another phi are deliberately outside this initial proof.
All other cases keep ordinary immutable concatenation.

Eligible appends call a compiler-private representation helper. It grows managed flat storage
geometrically, preserves Latin-1/UTF-16 units (including lone surrogates), and appends without
recopying the prefix while capacity remains. Ownership ends at every loop exit. Non-string
operands retain the ordinary addition/coercion path; actor-candidate execution also falls back.
Unused capacity remains part of the managed allocation after freezing; long strings can retain
up to approximately twice their payload size until collection. The collector accounts for the
reserved allocation, and no process-wide mutable state is added.
The string ABI and public built-in semantics are unchanged. Ownership metadata is initialized
before versioned-clone dispatch and shared across fast/generic transitions.

Nine shuffled, interleaved runs per binary on CPU 15, after every compilation finished, compare
unchanged sources built by e03aaf36 and the optimized compiler. Every complete output matches
Node for its own row count. Times include process startup, exclude compilation:

| Rows | Baseline median ms (min–max) | Optimized median ms (min–max) |
| --- | ---: | ---: |
| 1,000 | 36.83 (35.24–41.28) | 29.16 (28.84–37.13) |
| 2,000 | 92.24 (90.72–95.14) | 49.97 (49.27–51.03) |
| 4,000 | 310.14 (304.16–329.00) | 92.76 (92.22–94.55) |

The unchanged 2,000-row program takes 45.8% less wall time; at 4,000 rows it takes 70.1% less.
A separate interposer counts both dynamically dispatched `memcpy` and `memmove` requests:
187,921,452 bytes before versus 2,913,908 after at 2,000 rows (98.4% less observed traffic).
Optimized traffic at 1,000/4,000 rows is 1,481,247/5,779,448 bytes. Inline copies remain excluded,
and no interposer is used for timing. Regex replacement allocation remains the next target;
these results do not claim parity with Perry or replace the website's frozen release comparison.

Evidence: `.scratch/html/optimization-results.json`, `optimized-copy-counts.json`,
`accumulator-final-tests.log`, `native-accumulators.log`, `native-accumulators-stress.log`,
and the final gate evidence directory `target/gate-logs/run.C0b9e1SU/`. The backend proof/dispatch tests
and native parity probes cover snapshots, captured closures, exceptions, coercion, mixed encoding,
loop-exit aliases and allocation during growth. The private-string probe also passes with a
collection forced on every allocation.

The final routine gate passed all ten steps in 779 seconds with one fixed source snapshot:
3,006 workspace tests passed, 14 ignored, zero failures; native parity had 724 matches,
6 existing refusals and no divergences; all 17 available thirdparty bundles matched in both
engines (five bundles absent); routine Test262 had 97,745 passes across 102,522 selected
variants with no regressions across 122 chunks (116 extended variants deferred). Node, server,
regression and facts baselines passed, as did the performance gate for 12 routine bundles
(`postcss` performance deferred). This is routine coverage, not a complete extended run.
The accepted aggregate records code snapshot SHA-256
`131ec0cd35c183e03b346d6d0c1ba41a82c1f037666b26a3c20b316865a47288` and compiler SHA-256
`74edb834ecf427ce4c66887f36741bf668a14a9c0bdeeac4af160b1e89b5049e`.

The first gate exposed a test assertion that compared dispatch from an earlier function and
the expected addition to the private-helper surface snapshot. Both were corrected and pass in
the final run. The first aggregate correctly rejected changes made during that run; only the
final aggregate is accepted evidence.
