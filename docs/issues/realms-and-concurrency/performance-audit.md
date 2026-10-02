# Concurrency historical performance audit

This audit compares warmed work in owned checkouts. Builds finish before timing;
every result must match the reference checksum. It does not replace the concurrency
or parallel suite reports.

## Class polymorphism, 2026-09-29

Historical endpoint: `fa213915`, immediately before `fb335976` (issue 04 step 3).
Current production endpoint: `ffd1fcf6`. The intervening history contains shared-cache,
compiler, bootstrap and representation changes; this comparison establishes the total
regression, not its cause.

Both release CLIs built the unchanged `bench/non-io/class-poly.mjs` with
`--profile native-release` and clang 22.1.8 on the Ryzen 7 5800H Linux host. The
historical checkout uses the exact current timing harness (its only working-tree edit).
Harness SHA-256: `b041101125e8f8e789321983bd5ec1579c3a8a8d63abf584adaae43afdcaf827`.
Three alternating rounds ran after compilation finished, with load 2.08–2.38 on
16 hardware threads. All six native runs matched Node's checksum `393540687`.

| Variant | Warm median, ms | Three round medians, ms |
| --- | ---: | --- |
| Before step 3 | 1.90625 | 1.90625, 1.90625, 1.90625 |
| Current | 3.4375 | 3.4375, 3.4375, 3.4375 |

The current workload is **1.80×** the historical time. This contradicts treating
the earlier roughly 10% remaining slowdown as a current measurement. Attribution
and a fix remain open. Raw records: `target/perf-class-poly-paired.json`.

Binary SHA-256s:

- Historical: `bde903b5cf7d1e031e6284505e7c80759c4a76cbda5b5a6e3515043581a2fc34`.
- Current: `1b359d7c4f8ef5a7da8efd3e17e1d48cfc4db6d64ab3e6e9ecf179916225c5d3`.

The initial shared keys/shapes comparison is recorded below. Key-registration
bundle startup and structured-clone bootstrap comparisons remain owed.

### Guarded captured-array fix

The current generated loop had lost its captured-array entry guard. Its capture
was initialized once, but the initialization followed effectful class/module
setup, so the binding could not be proven initialized before every possible call.
The previous proof coupled entry specialization to unconditional dead-zone
elimination. Separating these facts restores specialization without changing the
generic path's checks.

The new proof accepts only a single explicit store by the binding's owner, in a
block outside every control-flow cycle. Parameters are excluded because they
already hold arguments before an explicit store. Uninitialization is permitted
only in the owner's entry block before that store. The entry guard rejects the
uninitialized sentinel; once it accepts an array, that binding cannot change.
Successor reachability is cached per owner/store block, including exceptional
edges. Mutable bindings and stores through another closure remain ineligible.

Three alternating rounds compared the historical, unchanged current, and patched
native-release binaries after all compilation finished. Every run returned the
same checksum `393540687`.

| Variant | Median of round medians, ms | Three round medians, ms |
| --- | ---: | --- |
| Before step 3 | 1.90625 | 1.90625, 1.90625, 1.9375 |
| Current without fix | 3.4375 | 3.4375, 3.4375, 3.5625 |
| Captured-array guard | 1.78125 | 1.75000, 1.78125, 1.78125 |

The patch cuts current warmed time by 48% and is about 7% faster than the
historical endpoint. This resolves the measured class-polymorphism regression;
it does not attribute every intervening change. Raw records:
`target/perf-class-poly-array-guard-paired.json`.

The executable probe `conformance/native/captured-array-entry-guard.js` checks
early calls that do and do not read the uninitialized binding, subsequent reads,
mutable bindings, loop writes, and a captured parameter updated by resuming its
generator while the reader is active. Node, interpreter and native-release agree.
Inspection of emitted release C confirms the immutable reader has the array
entry guard and the mutable, loop-written and parameter readers do not. The
probe explicitly selects native-release for future corpus runs.

After the fixed-snapshot gate passed on `a2444203`, a fresh build of the final
compiler repeated the historical/patched pair three times. Historical warm
medians were 1.875 ms in every round; patched warm medians were 1.750 ms in every
round, again with checksum `393540687`, at load 0.61–0.67 on 16 hardware threads.
This confirms the result after the final
parameter exclusion and safety-proof cleanup. Records:
`target/perf-class-poly-final-paired.json`.

## Arithmetic branch follow-up, 2026-09-29

The final `a2444203` release CLI built two otherwise identical loops of
300 million iterations: `if (i % 3)` and `if (i % 3 !== 0)`. Both print
`200000000`. After one warm-up, seven alternating runs had medians of
0.244620 s and 0.245033 s, respectively. Ranges were 0.243460–0.246237 s and
0.244428–0.245950 s. The roughly 0.17% difference is within the observed
variation, at load 0.58–0.61 on 16 hardware threads: the arithmetic condition has no measurable extra cost relative to
the explicit comparison. This confirms the existing numeric-twin follow-up on
the current compiler; no additional arithmetic-branch change was needed.

Timing is external and includes native process startup, which is common to both
variants and small relative to the loop. These are AOT variants, not a JIT
startup comparison. Builds and the gate had completed before measurement.
Records and exact input sources: `target/perf-arithmetic-audit/results.json`,
`target/perf-arithmetic-audit/truthy.mjs` and `comparison.mjs`.

## Executor split, 2026-09-29

Owned historical checkout endpoints: `30c1d548` immediately before, and
`036d288d` immediately after the executor split. Both built the unchanged
`bench/server/serve.mjs` and `app.mjs` with `--node --profile native-release`.
The historical Node load drivers are also identical between endpoints. Builds
finished before timing; load was 0.51–1.52 on 16 hardware threads.

Three alternating rounds ran one second of warm-up and five seconds of measured
traffic at 16 connections for each endpoint and workload. All twelve cells had
zero request errors and zero wrong responses. Server CPU uses the process's
Linux user+system tick delta divided by validated request count.

| Workload | Before, requests/s | After, requests/s | Before CPU, µs/request | After CPU, µs/request |
| --- | ---: | ---: | ---: | ---: |
| Hello | 16,635 | 16,563 | 17.312 | 17.086 |
| Mixed small service | 15,521 | 15,596 | 37.755 | 37.062 |

These are medians of three cells. Throughput differs by less than 0.5% in either
direction; CPU/request is slightly lower after the split, without a claimed
performance gain. The formerly load-obscured acceptance check is now complete:
the structural executor split did not regress these workloads.

The client and server share this host, so these are paired historical checks,
not peak-capacity claims about today's HTTP runtime. Port readiness checks
interrupted the first attempt and later paused the saved-cell run; only the
complete, persisted twelve-cell inventory is accepted. The runner resumed
missing cells without rerunning accepted ones and never killed an unowned process.
Evidence and runner:
[committed cell records and artifact hashes](performance-evidence/executor-split-2026-09-29.json).
The temporary runner is retained at
`~/Work/kiln-wt-perf-before-step3/target/perf-executor-pairs.py`.

## Shared keys and shapes, 2026-09-29

The isolated historical pair is `036d288d` to `8b0dc1e2`, with no intervening
commit. Both release CLIs built identical class-poly and JSON workloads under
native-release with the same current harness (SHA-256 above). The HTTP service
and client sources are also identical across endpoints. Compiles finished before
three alternating rounds per pair. Every checksum, response body and status matched.

| Historical workload | Before | After sharing |
| --- | ---: | ---: |
| Class-poly, warm ms | 1.875 | 1.90625 |
| JSON, warm ms | 76 | 80 |
| Hello HTTP, requests/s | 16,590 | 16,411 |
| Mixed HTTP, requests/s | 15,650 | 15,512 |

Class-poly's round medians overlap, but JSON's 5.3% slowdown is consistent across
all three pairs. HTTP throughput falls by roughly 1%, with CPU/request rising
from 17.085 to 17.305 microseconds for hello and 36.806 to 37.261 for mixed.
The historical object-spread benchmark cannot compile: its emitted C references
undeclared arithmetic variable `d19`. Keep that failure explicit; it supplies no timing.

The JSON and HTTP pairs were then repeated against current production `cdecff41`.
This comparison includes subsequent changes and establishes total regression,
not attribution to shared keys/shapes. The separate captured-array audit above
already establishes the current class-polymorphism improvement.

| Current pair | Before sharing | Current production |
| --- | ---: | ---: |
| JSON, warm ms | 75 | 62 |
| Hello HTTP, requests/s | 16,591 | 16,570 |
| Hello HTTP, CPU microseconds/request | 17.118 | 16.512 |
| Mixed HTTP, requests/s | 15,728 | 15,687 |
| Mixed HTTP, CPU microseconds/request | 36.954 | 35.753 |

JSON is about 17% faster than the original baseline. Current HTTP throughput differs
by less than 0.3%, while CPU/request is about 3% lower. These workloads have no
remaining overall regression. Load ranged from 0.97 to 2.26 on 16 logical CPUs;
HTTP client and server share the host, so these are paired checks rather than
peak-capacity claims. One port-availability interruption resumed only missing
saved cells. All 42 unique expected cells are validated before accepting the report.

The original change affected native C/provider state, so this audit measures native
workloads. It is a scoped hot-path check, not an exhaustive benchmark of every program.
The two remaining historical startup/clone audits are still owed.
Evidence: [cell records, source and executable hashes](performance-evidence/shared-keys-shapes-2026-09-29.json).
Raw output and runners remain under the historical worktree's `target/perf-shared-*`.

## Structured-clone startup, 2026-09-29

Exact historical pair: `fa213915` -> `c3915a76`, the single structured-clone commit.
Both owned checkouts built the same small hello and typed-array sources with
`native-release`, clang 22.1.8 and Rust 1.97.1. The interpreter ran with `--one-shot`
to avoid a resident daemon. Timings use fresh processes and a warm filesystem,
after all goal builds/tests finished. Each of three alternating rounds used 21
native samples or five interpreter samples, checking exit status and exact stdout
and stderr every time. These are external elapsed times and binary sizes, not
hardware instruction/cycle counts or isolated bootstrap phase timings.

| Workload | Before, ms | Immediately after, ms |
| --- | ---: | ---: |
| Native hello | 2.084 | 2.066 |
| Native hello, Node adapter | 3.123 | 3.149 |
| Native typed-array loop, 10,240,000 reads | 97.882 | 93.689 |
| Interpreter hello | 76.563 | 78.627 |
| Interpreter hello, Node adapter | 92.638 | 99.420 |
| Interpreter typed-array loop, 1,024,000 reads | 1,110.019 | 1,389.472 |

The interpreter loop regressed about 25% at the historical commit, confirming a
material cost that the earlier loaded-machine measurement could not settle. The
stripped native hello grew from 9,335,288 to 9,579,192 bytes (+238 KiB); with the
Node adapter it grew from 13,062,744 to 13,542,776 bytes (+469 KiB). Native startup
and the selected native loop did not show the interpreter's regression.

A second three-round comparison pairs the same pre-change sources/binaries with
current `b62170d1`. It establishes the total current result, not attribution to
any one intervening change:

| Workload | Before clone commit, ms | Current, ms |
| --- | ---: | ---: |
| Native hello | 2.153 | 1.925 |
| Native hello, Node adapter | 3.131 | 3.672 |
| Interpreter hello | 76.484 | 53.213 |
| Interpreter hello, Node adapter | 93.218 | 68.458 |
| Interpreter typed-array loop, 1,024,000 reads | 1,111.672 | 730.666 |

The selected interpreter regression is absent in current production: the loop
is about 34% faster than before the clone commit, and both interpreter startup
rows are faster. Current native hello is 5,305,288 stripped bytes; Node hello is
17,574,008 bytes. Ordinary hello is smaller and starts faster than the historical
endpoint, while Node hello has grown and starts about 0.54 ms slower. The latter
includes the intervening Node/actor surface changes and remains a bootstrap
startup/density concern under issue 08; this comparison does not attribute it to
structured clone. All 66 result cells, sample lists, source text and binary
digests are in
`performance-evidence/structured-clone-startup-2026-09-29.json`.

## Key-registration bundle startup, 2026-09-29

Exact single-commit pair: `0803cd49` -> `9207eeb6`. Both use the identical
1,226,958-byte Babel bundle (SHA-256
`0ee198a74ab544a7f5ffabdb5bc1b0033a39ce5a24b5c08bedfe576afd722479`)
and `--node --profile native-release`. Every native run matches Node's exact
stdout, stderr and exit status. Three alternating rounds of 21 fresh-process
samples, after compilation finished, gave median startup-plus-work times of
63.825 ms before and 65.228 ms after; round medians were 63.630/66.941/63.825 and
65.596/65.228/64.673 ms. The ranges overlap, so this row does not establish a
regression. It includes bundle work and is not a constructor-only timer.

The 9,947,697-byte TypeScript bundle (SHA-256
`12f46f775be36282c8a053a8854276df22d1bf94c1cd3dcae2bafe3a627f2257`)
produced no executable at either endpoint under a 6 GiB address-space cap. A
post-change retry at 10 GiB also aborted on allocation failure, with maximum RSS
9,054,824 KiB; `native-release-size` at 6 GiB failed too. These historical build
failures are retained, not turned into startup numbers. The TypeScript startup
comparison remains unmeasured; Babel alone cannot discharge that named row.
Build commands, failure diagnostics, source digests and all six Babel cells are
in `performance-evidence/key-registration-startup-2026-09-29.json`.
