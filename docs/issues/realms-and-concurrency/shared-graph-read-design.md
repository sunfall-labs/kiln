# Shared graph reads at the actor boundary

Status: architecture checkpoint, 2026-09-29. The public API and safety contract are in ADR
0073. This document records the native read-path work prompted by the public actor benchmark.

## Keep the data paths explicit

An ordinary actor argument uses structured clone. `share(value)` creates one immutable, sealed
graph and messages carry a strong handle to it. `transfer(buffer)` moves its backing store, and
`prepareTransfer(value)` snapshots a graph into a private arena before its ownership handoff.
Direct `isolate(factory)` still needs an incoming-alias proof. These paths have different costs
and guarantees; the shared graph must not silently become a per-actor clone for speed.

The graph arena is the semantic and storage owner. It contains no pointers into a realm and is
published only after sealing. A realm holds a facade with its own stable identity and a strong
graph lease. The arena is reclaimed after the last handle and the scheduler's quiescent point.
Immutable data may be read by concurrent realms; mutation, prototype overlays and any caching
belong to the reading realm. Ordinary mutable objects retain their existing fast path.

## What the first public measurement found

`bench/concurrency/js/actor-shared.mjs` starts actors before timing a roughly 1 MB graph. It
checks a complete traversal on load and on query. A 2026-09-29 native-release run, after adding
an arena field index, gave the following preliminary numbers on this host (one run per cell):

| held actors | mode | load all actors | query all actors | incremental held RSS |
| ---: | --- | ---: | ---: | ---: |
| 4 | clone | 81 ms | 7.5 ms | 48 MB |
| 4 | shared | 664 ms | 227 ms | 33 MB |
| 16 | clone | 306 ms | 22 ms | 184 MB |
| 16 | shared | 2,640 ms | 1,218 ms | 95 MB |

The sealed hash index removes a linear scan across 6,000 array fields per indexed read. It is
necessary for this representation, but insufficient for fast user reads. The native path still
retains a graph lease and then makes separate provider calls for node info, indexed lookup and
field contents. Each call looks up a lease under a process mutex and clones an `Arc`. String
values become fresh realm strings on reads; child nodes need facade lookup. The next bottleneck
must be confirmed with a profile, but these costs are visible in the code and occur for every
property access in the benchmark. Issue 09's scheduled-worker exit-scope bug, which made some
16-actor stops fail after successful queries, was fixed before the next measurements.

## Profile checkpoint: weak facade identity lookup

A `native-profile` run of the public 16-actor workload found a second linear scan, this time
through every live shared facade in a realm. `kiln_c99_new_shared_facade` was called about
490,000 times and held 61.85% of the sampled self time in that instrumented run; contended
mutex acquisition accounted for another 8.5%. The instrumented percentages are diagnostic,
not release timing, because `-pg` adds a call at each function entry.

A realm-local weak hash index now finds a facade by graph generation and node. It keeps the
existing weak list for collection and removes the exact index entry in the facade finalizer;
no table edge is traced as a GC root. Four interleaved native-release runs at 16 actors gave
these medians on the same host and workload:

| representation | load all actors | query all actors | incremental held RSS |
| --- | ---: | ---: | ---: |
| weak list | 2,666 ms | 1,218 ms | 95.4 MB |
| weak hash index | 408 ms | 1,060 ms | 102.5 MB |

The index cuts load latency about 6.5x while retaining a substantial memory advantage over
per-actor clones. Query remains much slower than the clone row above. Its next candidate is the
read-only arena view below; the repeated retain/info/find/field/release calls and string
materialization remain on every property access. Profile that path again after the index
change before choosing the first view operation. Keep the view only if it improves public query
latency without erasing the held-memory advantage.

Validation: the backend crate passed 253 unit cases and 36 emitted-C snapshots; ordinary native
parity reported 352 matched, 1 known refusal and 0 divergences. The new wide-facade probe matched
both engines separately. Three shared probes, including the wide probe, matched at 1, 2 and 8
simultaneous realms under every-allocation GC stress. The full native-stress corpus reported
313 matched, 1 known refusal and 0 divergences, with 40 declared stress skips.

## Native read design to test

Provide a read-only graph view to the C runtime for the lifetime of a rooted facade or explicit
turn-scoped lease. The view exposes validated, fixed-layout node, field and index records from
the sealed arena. One boundary operation acquires the view; repeated reads use its read-only
bytes directly, with bounds checks and a key comparison on hash collisions. Keep arena layout
definitions and validation in one Rust owner and generate or assert the C layout at the ABI
boundary. Do not let a pointer to a realm heap enter the arena, and do not let an arena pointer
survive the view's strong lease. The GC and scheduler need an explicit proof that they cannot
retire the arena while an emitted read, reflection walk or host callback holds the view.

Use realm-local, weak identity caching for child facades. Repeated string reads need an immutable
string view backed by the sealed arena, or another bounded way to avoid allocating and copying
the same bytes on every read. The string's lifetime must hold the arena, and UTF-16 semantics,
enumeration order, descriptor reads and prototype behavior must match ADR 0073. Avoid filling
per-realm caches with the whole dataset: that would erase the held-memory benefit. Keep the
ordinary object and array layout untouched.

First profile the current path and test a narrow view for array index, record field and string
reads. Compare interleaved clone/shared actor runs at 4 and 16 actors, plus a larger graph. Keep
the view only if load/query time improves materially while held RSS remains substantially below
clones. Run the native parity, stressed GC, concurrent-realms and reflection probes before
enabling it by default. A one-call provider lookup is an acceptable intermediate experiment,
but it still crosses the mutex and materializes values for every property read; measure it
against the read-only view rather than declaring it the end state.

## Direct-view checkpoint, 2026-09-29

The narrow native view is implemented and validated. A
facade owns one strong graph lease and a pointer to the sealed arena; C reads the fixed-layout
node, field and index records directly with bounds checks. An arena layout version and C/Rust
size/offset assertions guard the ABI. A read keeps its facade live through any allocation, and
facade finalization releases the lease. External graph edges still use the provider to resolve
their separate handle. This view does not change mutable object layouts or the public API.

Four interleaved native-release rounds of the same public `actor-shared` workload gave these
medians; every round produced the same checksum. The old binary is the weak-hash-index commit
above, and both binaries ran on this host during the same measurement window.

| held actors | representation | load all actors | query all actors | incremental held RSS |
| ---: | --- | ---: | ---: | ---: |
| 4 | provider calls | 101.1 ms | 160.1 ms | 35.5 MB |
| 4 | direct view | 62.1 ms | 21.6 ms | 35.7 MB |
| 16 | provider calls | 402.3 ms | 1,065.7 ms | 103.0 MB |
| 16 | direct view | 246.6 ms | 127.1 ms | 105.9 MB |

The improvement justifies keeping the direct view if the stressed checks pass. The remaining
query cost includes constructing realm strings from arena bytes on each read; a borrowed string
representation is a separate change with a harder lifetime and GC contract. Keep that out of
the initial release unless a public workload still needs it. `Arc` ownership through the facade
is sufficient for a single graph view; an epoch library would add another lifetime protocol
without removing the need to root facades during allocations. The scheduler's quiescent retire
path still applies when the last graph handle is released.

Validation on the combined final tree: the repository gate passed all 10 steps, with 2,282 crate
tests and 97,116 routine Test262 passes and no chunk regressions. Native parity reported 353
matched, 1 known refusal and 0 divergences. Five shared probes matched at 1, 2 and 8 realms
under every-allocation GC stress; the complete native-stress corpus reported 313 matched, 1
known refusal and 0 divergences, with 40 declared stress skips. A nested shared graph containing
a BigInt exercises the external-edge provider fallback.
