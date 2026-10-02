# 13. Benchmarks against Node, Bun and Go

Status: resolved (three-round non-server and HTTP comparisons refreshed; capacity targets measured)
Blocked by: 01 (then extended as each issue lands)

Under `bench/`, following `CLAUDE.md`'s rules (warm, interleaved, checksums compared):

- worker start time and memory: a kiln realm, a Node worker, a Bun worker, a goroutine;
- message throughput and latency, small and large payloads, copy and transfer;
- sharing a large read-only dataset across N workers;
- many-realm scaling: 10, 1,000 and 100,000 realms or goroutines doing a small periodic task;
- server scaling: thread mode against process mode against Node `cluster` against Go `net/http`.

## Acceptance

- One command reproduces the table, with machine details, and it is updated as each issue lands.

## Comments

### 2026-09-29: socket-capable three-round HTTP refresh

`bench/concurrency/results-2026-09-29-server.md` records three interleaved rounds
on `6696a3a1`, after the program gate and collector stress finished. All seven
configurations completed without request errors; no cell exceeded the 25% spread
threshold. The 128-connection hello-world workload measured kiln process and thread
modes at **381k requests/s** each, Bun at **376k/s**, Go at **266k/s**, and Node
cluster at **202k/s**. Kiln single-worker measured **111k/s**, Node single-process
**39,557/s**. Process/thread tree PSS was **157/147 MB**; their p99 latencies were
**1.86/1.69 ms**. Raw records are retained in
`target/bench-concurrency/2026-09-29T121422-0600/raw.json`.

The load generator shares the server's machine and competes for its cores. Recorded
load was 1.2 before the first job and 11.3 median across jobs, reflecting the preceding
all-core workloads; no build or correctness lane ran concurrently. These are
same-host high-concurrency results, not universal throughput claims. They do not
remeasure the earlier 16-connection thread-mode regression or crash isolation, so
process mode remains the default. Together with the non-server report, this completes
the suite refresh; bootstrap density and the historical hot-path audit remain separate.

### 2026-09-29: three-round Node, Bun, Go and kiln refresh outside HTTP

`bench/concurrency/results-2026-09-29-nonserver.md` records three interleaved rounds of
worker and actor startup, cross-realm and in-realm messaging, shared datasets, timed tasks,
parallel map and scheduled realms. The native-release binaries came from `a525e7c4`; the
report's dirty marker is the benchmark runner's memory-rule wording and README edit. The
raw 387 records are in `target/bench-concurrency/2026-09-29T054623-0600/raw.json`.
There were no build failures, run errors or checksum divergences. Eighteen cells exceeded
the 25% noise threshold; the median observed load was 2.9 on 16 hardware threads.

The median public capacity row held 10,000 actors at 5,943 MB with 18 threads, took
7.40 seconds to ready, 9.06 seconds to call each once, and 2.63 seconds to stop all.
Each round checked the 50,005,000 reply checksum. The scheduled 100,000-realm row
completed in 7.91 seconds at 501 MB peak RSS, with every realm's 10-turn output and
status checked. The actor's warm spawn-to-ready median was 3.05 ms. Kiln's cross-thread
small-object round trip was 41.5 µs against Node's 18.5 µs, its 1 MB copied graph
send and acknowledgement 11.4 ms against Node's 5.18 ms, and its heavy parallel map
83.8 ms against Go's 74.0 ms and Node's worker pool's 231 ms. These are same-host
measurements, not universal speed claims. The full comparison still needs the HTTP server
workload in an environment that permits loopback sockets.

### 2026-09-29: 100,000 scheduled realms complete one million turns

The scheduled-realm launcher had silently capped `KILN_REALMS` at 10,000, even though the
fixed lane pool already supports more. After raising only that cap to 100,000, the
native-release periodic workload completed 10 timer turns in each of 100,000 realms in
8.22 seconds, with 502,276 KiB peak RSS and all 100,000 individual outputs and statuses
checked. Its checksum was 1,000,000 turns. The same binary and runner measured 1,000
realms in 99.4 ms at 73,824 KiB and 10,000 in 767.9 ms at 137,168 KiB, each with the
expected output count. The 10,000-to-100,000 peak-RSS slope is about 4.1 KiB per added
parked scheduled realm. The benchmark runner now projects memory before the large row
and watches the child process while it runs. Raw rows are in
`target/realm-100k-capacity.jsonl`. This establishes the top-level scheduled-realm
capacity target on this host; public actors carry a separate, larger Node/module bootstrap.

### 2026-09-29: 10,000 public actors held and checked

After promoted nursery page reclamation, the native-release public capacity binary held
5,000 actors at 3,014,108 KiB RSS and 10,000 at 6,006,560 KiB. The latter used 18 threads,
spawned to ready in 7.38 seconds, called each actor in 8.92 seconds and stopped them in 2.54
seconds; the 10,000 replies summed to the required 50,005,000. A monitored wrapper sampled
6,520,440 KiB peak RSS while preserving at least 2 GiB host available memory. A second run
through the benchmark runner reproduced the 10,000-actor row at 6,006,916 KiB held RSS,
18 threads and the same checksum.

The runner's former `projected * 1.5 < 70% of available` gate would still skip this now
measured-safe row. It now requires projected RSS plus the larger of 2 GiB or 20% reserve to
fit available memory, and it watches child RSS and remaining memory while the row runs.
The guard terminates only that child tree if the reserve is consumed. A 9 GiB address-space
limit was tried first and the program returned out of memory before reporting a result, so
the runner uses resident-memory observations for this capacity gate. The current output is a
same-host capacity result, not a claim that 10,000 actors meet the tens-of-KiB density target.
The complete Node/Bun/Go comparison remains due.

### 2026-09-29: direct factory graph construction and handoff

The public actor graph benchmark now has a third path, `isolate(buildGraph)` followed by
`transfer(handle)`. It reports construction and handoff separately: unlike clone and
`prepareTransfer`, the direct factory must create a fresh ~1 MB graph for each sample. Seven
fresh-process native-release rounds, ten alternating samples each, checked the same sum in all
paths. Medians were 8.31 ms direct construction, 17.54 ms direct handoff plus reply, and
25.89 ms total. The prebuilt-graph clone call was 12.93 ms; prepared snapshot and handoff were
4.72 and 4.93 ms, or 9.78 ms total. The direct handoff's 17.54 ms includes ownership
certification across the candidate and sender heap, so zero payload copy does not yet make it
the fastest public path. Raw rows are in
`target/gate-logs/architecture-check/actor-graph-direct.jsonl`. The benchmark report now labels
the differing construction costs explicitly. This is a narrow same-host measurement, not the
full concurrency refresh.

### 2026-09-29: sealed arena direct-read comparison

Four interleaved native-release rounds compared the weak-hash-index baseline with a direct
read-only view of the sealed arena. The 16-actor shared-graph load/query medians changed from
402.3/1,065.7 ms to 246.6/127.1 ms; held RSS changed from 103.0 to 105.9 MB. At four actors,
load/query changed from 101.1/160.1 ms to 62.1/21.6 ms, with essentially the same held RSS
(35.5 versus 35.7 MB). Every round's checksum matched. The new path still materializes realm
strings, so a read-heavy shared traversal remains slower than the preliminary 16-actor clone
query row (22 ms), while using much less held memory (about 106 versus 184 MB). The safety and
layout boundary are recorded in `../shared-graph-read-design.md`.

### 2026-09-29: shared facade lookup profile and read-path checkpoint

With issue 09's concurrent-stop bug fixed, four interleaved native-release runs of the same
16-actor public shared-object workload compared the weak facade list with a realm-local weak
hash index. Median shared load/query time changed from **2,666/1,218 ms** to
**408/1,060 ms**; incremental held RSS changed from **95.4 MB** to **102.5 MB**. The checksum
matched in every run. A `native-profile` run before the index recorded about 490,000 facade
lookups and attributed 61.85% of its instrumented samples to their linear scan. The release
timings, rather than the instrumented share, are the performance evidence. Query remains far
behind the earlier 16-actor clone row (22 ms), so the sealed arena read path remains open.
The design and the next measurement gate are in `../shared-graph-read-design.md`.

### 2026-09-29: public actor shared-object benchmark

The shared-dataset workload now measures one ~1 MB frozen object graph through the public
actor API, separately from the `SharedArrayBuffer` byte-table rows. In fresh processes, it
starts four actors before measuring one `share()` snapshot, delivery to all actors, a checked
graph walk during each delivery and one later checked query per actor. A quick interleaved
`--only shared` report had no failures or divergences: clone loaded in 82.6 ms and queried in
6.92 ms at a 47.1 MB held-RSS increment; sharing took 19.1 ms to snapshot, 671 ms to load and
228 ms to query at a 32.5 MB increment. A separate 16-actor direct run found 306/22 ms
clone load/query against 2,640/1,218 ms shared, and 184/95 MB incremental RSS. These are
preliminary one-round numbers, not a performance baseline. Repeated 16-actor shared runs
intermittently failed after their queries, during concurrent stops; issue 09 now records and
fixes that scheduled-worker exit-scope problem. The benchmark exposed an actor message bug: its
preparation walked a shared facade into an ordinary array. The fix preserves shared handles through actor
calls and replies,
with interpreter/native, stressed-GC and simultaneous-realm probes. The native sealed field
index removes the former 6,000-field scan, but the per-read provider lease path is still
expensive. The next read design and acceptance criteria are in
`../shared-graph-read-design.md`.

### 2026-09-29: 1 KiB and 100 MiB buffer transfer comparison

The message workload now reports a transferred-buffer round trip at both sizes, with sender
detachment and boundary-byte checks. Seven interleaved rounds at native-release measured kiln
at 0.0802/0.0800 ms, Node at 0.0170/1.6157 ms, and Bun at 0.0177/0.5321 ms (1 KiB/100 MiB).
The focused benchmark command rendered both rows once, under the cross-realm table, without
divergences. The measurement isolates ownership handoff from a payload scan; it does not time
allocation or initialization of the 100 MiB buffer. Raw records and the report are under
`~/.cache/kiln/buffer-transfer-size-2026-09-29/`.

### 2026-09-28: public actor capacity exposes the worker bootstrap cost

`bench/concurrency/js/actor-capacity.mjs` now keeps 10, 100 or 1,000 public `spawnActor`
realms alive, using at most 64 pending requests at once. It calls each actor, checks each reply,
then stops every owner. The quick `--only start` report rendered all three rows with no failures:

| Held actors | Spawn to all ready | One call each | Held RSS | Threads |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 6.43 ms | 1.63 ms | 30.5 MiB | 18 |
| 100 | 37.1 ms | 13.7 ms | 103 MiB | 18 |
| 1,000 | 382 ms | 185 ms | 819 MiB | 18 |

The direct 1,000-actor result checked checksum 500500 and all stops. The 100-to-1,000 held-RSS
slope is about 816 KiB per public actor, far above issue 08's roughly 97 KiB core-realm slope.
The full runner projects RSS from smaller public counts and skipped 10,000 actors on this host:
about 7.8 GiB projected, outside its 70%-of-available-memory rule with a 1.5× margin. This is
a measured capacity limit, not a successful 10,000-actor result. Raw records and the quick
report are under `~/.cache/kiln/actor-capacity-2026-09-28/`. The separate 100,000 *top-level*
realm row also remains open.

### 2026-09-28: actor call and reply plain-graph guard

The public actor graph workload now measures the guarded single-clone call and reply paths.
Seven runs of ten alternating samples, with matching checksums, gave 12.35 ms ordinary call,
19.37 ms copied graph round trip, and 10.35 ms complete prepared transfer. The earlier
same-workload ordinary call at `fed40541`
was 75.67 ms;
worker `postMessage` already used the shared Rust codec and did not pay the actor's temporary
TypeScript copy. Seven alternating old/new 20-actor start rounds gave 2.423/2.404 ms warm
spawn and 0.285/0.289 ms first-call medians. The remaining gap between actor clone and the
worker message row is the actor's ticket-free graph scan and call envelope, not a second graph
copy. Raw measurements are under `~/.cache/kiln/actor-call-codec-2026-09-28/`.

### 2026-09-28: public actor graph-call modes

The concurrency runner now measures one 1 MB graph through `ActorRef.call` with ordinary clone
and through `prepareTransfer` with a private arena. It reports preparation, handoff plus reply,
and the combined prepared cost; actor startup is outside the clocks. Seven runs of ten
alternating pairs on one native-release binary gave matching checksums and medians of 75.67 ms
for clone and 9.39 ms for the complete prepared path (4.79 ms preparation, 4.63 ms handoff and
reply). A quick `--only messages` run verified report integration. This is a kiln-only public API
comparison, not a Node/Bun actor equivalence. The default actor clone path's cost is now a
measured follow-up to the worker messaging results in issue 21. Raw measurements and logs are
under `~/.cache/kiln/actor-graph-2026-09-28/`.

### 2026-09-28: direct actor spawn baseline

The start workload now measures the public `spawnActor` path separately from the Node-compatible
`Worker` path. The native actor worker already uses a process-wide bounded scheduler by default;
ordinary Node workers still use dedicated threads. Three start-only rounds on a native-release
binary at `77c3c759`, with twenty fresh actors per round, found 2.80 ms median from `spawnActor`
to ready, 3.84 ms for the first actor, and 0.29 ms for the first method reply. The separate
Worker first-reply median was 2.02 ms. The actor row has no Node/Bun equivalent and does not
claim that its 0.78 ms gap is a single cost: its path includes actor module evaluation, mailbox
registration, constructor validation, and a ready message. Full start-workload raw records and
the generated report are in `~/.cache/kiln/actor-start-2026-09-28/`. Reproduce with
`python3 bench/concurrency/run.py --only start --kiln target/release/kiln` after building the
release CLI.

### 2026-09-28: full concurrency refresh after scheduled actors and graph handoff

`bench/concurrency/results-2026-09-28.md` records three interleaved rounds at `8bda95ef` with
machine, versions and per-workload tables. The raw 339 records are under
`~/.cache/kiln/realm-bench-2026-09-28/raw.json`. All programs built, checksums matched, and nine
cells exceeded the report's 25% noise band. Median
observed load before jobs was 5.8 on 16 hardware threads (range 2.2–12.5).

Selected medians: kiln worker first reply 2.15 ms versus Node 20.9 ms; cross-realm small-object
round trip 39.2 µs versus Node 18.7 µs; 1 MB copied graph 11.0 ms versus Node 5.03 ms; and a
transferred 16 MB buffer 0.0796 ms versus Node 0.0371 ms. The separate scheduled-realm row
completed 1,000 realms in 127 ms at 172 MB peak RSS. Its measured slope projects about 14.9 GiB
for 100,000 realms, so the harness correctly skipped that row against 10.7 GiB available. This
is a measured capacity limit on this machine, not a 100,000-realm success result. Issue 08's
remaining per-realm memory is the prerequisite for that acceptance row.

The separate one-realm-per-connection probe also ran three rounds from the same native-release
binary. At 500 connections, scheduled realms completed in a median 208 ms with 18 peak threads,
versus 215 ms with pinned realms and about 487 peak threads. Server-observed response p95 was
108 ms scheduled versus 2.46 ms pinned; the startup burst's latency distribution remains issue
09 work. Its raw data is `~/.cache/kiln/realm-bench-2026-09-28/realm-connections.json`.

### 2026-09-26: the suite and the first baseline

`bench/concurrency/run.sh` (`--quick` for one round) reproduces the table; `bench/concurrency/README.md`
explains every workload and what "not available" means. Full results, machine details and the
per-workload tables: `bench/concurrency/results-2026-09-26.md` (3 interleaved rounds at `1e46b859`,
7m47s, load average median 20.5 on 16 threads -- noisy, read to one significant digit).

| workload | node | bun | Go | kiln today |
| --- | --- | --- | --- | --- |
| start a worker, first message back (warm) | 43 ms | 19 ms | 0.8 µs | not available; nearest: `KILN_WORKERS=1` process spawn-to-exit 9.0 ms (node process 33 ms, bun 9.4 ms) |
| memory per added worker, 100 alive | 4.5 MB | 3.2 MB | 3.4 KB | not available; `KILN_WORKERS` process: 1.6 MB PSS per added process |
| small message ping-pong, across threads | 65 µs | 28 µs | 0.3 µs | not available; in-realm `MessageChannel` 148 µs (node 25, bun 9.3) |
| small messages one-way | 246k/s | 578k/s | 17.9M/s | not available; in-realm 8.4k/s (node 126k, bun 486k) |
| 1 MB object graph send + ack | 15.5 ms | 6.7 ms | 0.67 ms (deep copy) / 0.014 ms (ref) | in-realm 175 ms |
| 16 MB ArrayBuffer round trip, copy / transfer | 45 / 0.09 ms | 20 / 1.2 ms | 2.4 / 0.03 ms | in-realm 18 / 11 ms |
| 50 MB table, 16 workers: setup copy / SAB, query, RSS | 677 / 0.4 ms, 116-135 ms, 312-1,112 MB | 719 / 0.3 ms, 110-130 ms, 153-1,691 MB | 0 ms, 82 ms, 59 MB | not available |
| 100,000 tasks x 10 timer ticks: wall, peak RSS | 1.2 s, 418 MB | 0.57 s, 196 MB | 0.30 s, 208 MB | 2.9 s, 732 MB (async tasks, one realm) |
| map 10M, heavy callback: sequential / all cores | 991 / 349 ms (pool) | 640 / 407 ms (pool) | 723 / 104 ms | 952 / 1,144 ms (`parallelMap` falls back to sequential) |
| map 10M, light callback: sequential / all cores | 120 ms / -- | 62 ms / -- | 48 / 17 ms | 56 ms / build failed (see below) |
| hello-world HTTP, all cores: rps, p99, tree PSS | 88k (`cluster`), 12.5 ms, 554 MB | 177k (`reusePort`), 3.6 ms, 107 MB | 153k, 5.7 ms, 18 MB | 281k (`KILN_WORKERS=auto`), 3.7 ms, 172 MB |

Caveats:

- Noise: 91 cells spread by more than 25% across rounds; a quick run at load 5-12 gave the same
  ordering with smaller times. `oha` shares the cores with the servers.
- `parallelMap`'s typed pool does not compile at `1e46b859`: `call_method.rs`'s pool guard names
  `kiln_inheriting_new_target` (now `kiln_c99_inheriting_new_target`). With that renamed in a
  scratch build (not committed), the light map runs in ~19 ms vs ~119 ms at `KILN_PARALLELISM=1`.
  The heavy callback (a loop) is not accepted by the pool at all.
- A second emitter bug: `{ code: i * 3 }` inside a loop emits an undeclared `d<n>` temporary;
  worked around in `js/common.mjs` with `(i * 3) | 0`.
- kiln's in-realm `MessageChannel` is 10-60x slower than node/bun's, and transfer costs about as
  much as copy.
- Not measurable yet (marked "not available" in the table): kiln workers, cross-realm messages,
  sharing a dataset across workers; many-realm scaling is measured as many async tasks in one
  realm. These rows fill in as 05-10 land.

### 2026-09-27: refreshed after workers, shared buffers and parallel built-ins

`bench/concurrency/results-2026-09-27.md` is a full three-round run from a fresh release CLI at
`7b01548c`. The report's dirty marker reflects benchmark harness and documentation edits only;
the CLI and native runtime source were at `7b01548c`. It took 6m35s including native compilation;
load median 6.8 on 16 hardware threads, with 10 noisy cells. All 330 records completed: no build
failures, run errors or checksum divergences. The raw records and full log were preserved under
`~/.cache/kiln/realm-bench-2026-09-27/` after the measuring worktree's run.

The runner now refreshes the release CLI every invocation and uses the same `node:worker_threads`
start and messaging programs in kiln as in Node and Bun. It adds kiln's copied and shared
`SharedArrayBuffer` cases for the 50 MB table, plus `KILN_WORKERS_MODE=thread` in the server row.
The README and generated report label the remaining gaps.

| selected median | node | kiln |
| --- | ---: | ---: |
| Worker first reply (warm) | 20.6 ms | 1.70 ms |
| 100 live workers, RSS per added worker | 4.36 MB | 0.466 MB |
| Cross-realm small-object round trip | 18.4 µs | 68.1 µs |
| Cross-realm 1 MB graph | 5.05 ms | 30.4 ms |
| 16 MB buffer transfer round trip | 0.0373 ms | 0.909 ms |
| 50 MB shared table to 16 workers: setup / RSS | 0.300 ms / 312 MB | 0.625 ms / 85.2 MB |
| 100,000 async tasks in **one** realm: wall / peak RSS | 1,067 ms / 415 MB | 739 ms / 296 MB |
| All-core HTTP, process mode | 200k/s / 549 MB PSS | 372k/s / 163 MB PSS |

At 128 connections, kiln thread mode was 378k/s and 151 MB PSS against process mode's 372k/s
and 163 MB. This does not remeasure the prior 16-connection deficit or the crash-isolation
tradeoff, so it does not change issue 07's process-mode default. The shared-table row shares
bytes, not user object graphs in the immutable region; that awaits issue 11. The 10, 1,000 and
100,000 *separate realms* workload awaits the M:N scheduler (09); the one-realm async task proxy
cannot satisfy that part of this issue's acceptance. Keep this issue open and refresh as those
features land.

### 2026-09-28: separate native realm scaling harness

`bench/concurrency/run.sh --only realms` now builds a native module whose ten timer turns each
print `10`, runs it as 10, 1,000 and (if memory permits) 100,000 separate scheduled realms, and
checks every realm's output and exit status. It records whole-process wall time and peak RSS,
including the launcher's two result files per realm; it does not present this as equivalent to
Node/Bun async tasks in one realm. The large row projects RSS from the smaller two counts and
skips when that would exceed 70% of available host or cgroup memory with a 1.5× margin.

A one-round `--quick --only realms` smoke run validated the harness at 10 and 1,000 realms; the
machine load was 23 on 16 hardware threads, so the numbers were not recorded as a performance
baseline. A direct run of the full count list projected about 21 GiB for 100,000 core realms on
this 15-GiB host and skipped that row. The all-workload refresh, quiet-machine comparisons, and
100,000-realm capacity result remain open.
