# Concurrency benchmarks: kiln against node, bun and Go

One command reproduces the table:

```sh
bench/concurrency/run.sh            # 3 interleaved rounds, about 8-10 minutes
bench/concurrency/run.sh --quick    # 1 round, fewer sizes, about 3 minutes
bench/concurrency/run.sh --only messages,server
bench/concurrency/run.sh --only realms --quick
```

The separate one-realm-per-connection scheduler probe is
`python3 bench/concurrency/realm-connections.py --quick` after building the release CLI with
`mbx build --release -p kiln-cli`. It compiles one native TCP client outside the measured runs,
serves loopback replies from one Python reactor, and compares Node workers, the original pinned
realm launcher, and the turn scheduler. On Linux it records peak process threads and RSS from
`/proc`; every client must return the same checksum. The full form runs three interleaved rounds
at 1, 10, 100 and 500 clients (Node stops at 100), with raw records under
`target/bench-concurrency/realm-connections.json`. `--lanes N` fixes the scheduler budget.
`--counts N ...`, `--rounds N`, and `--runtimes node pinned scheduled` select a smaller
diagnostic run. The raw records include the first accept, last response, response span,
median accept, and median and 95th percentile of server-observed response time, so a slow
connection can be separated from launcher teardown. The Python server uses a 4,096-connection
listen backlog: its former default of 100 caused a long tail at 500 concurrent clients.
`--backlog N` is available to reproduce that bottleneck.

It refreshes the release `kiln` with `mbx build --release -p kiln-cli` (unless `--kiln` selects
an explicit binary), builds the kiln
programs with `--node --profile native-release` and the Go program once, outside every
measurement, and writes `results-<date>.md` here with the machine details beside the numbers. The
raw records (every round, every metric, the load average before each job) go to
`target/bench-concurrency/<timestamp>/raw.json`. It needs `node`, `bun`, `oha` and `python3` on
`PATH`; Go comes from `PATH` when a working `go` is there and otherwise from
`mise exec go@latest`, which downloads into mise's cache without changing any configuration.

This is issue 13 of `.scratch/realms-and-concurrency/`: the suite is extended as each issue lands,
and a "not available" cell becomes a number when the feature it names exists.

## How it measures

The rules are `CLAUDE.md`'s "Benchmarking: time the work, not the launcher":

- **Warm, repeated, inside the process.** Every program times itself with `performance.now()`,
  runs a cold iteration first and reports it apart, and reports a median of warm iterations.
  Process start is only ever measured where process start is the question (the `KILN_WORKERS`
  rows).
- **Interleaved rounds, medians.** Each round runs every job once, rotating the runtime order, so
  a background build that starts mid-run hurts every column. A cell is the median across rounds;
  a leading `~` marks one whose rounds spread by more than 25% of their median. The machine is
  shared, so the load average before each job is recorded and summarised in the results.
- **Answers are compared.** Every case prints a checksum (a sum of the replies, of the lookups, of
  the mapped values). A cell whose checksum differs from the reference runtime's (Go where it runs
  the case, else node) did different work: it is excluded and named under "Divergences", never
  shown as a time. A fast wrong answer reads exactly like a win.
- **Memory from the right place.** In-process `VmRSS` deltas for workers that are threads (node,
  bun, Go); the process's peak resident set (`wait4` rusage) for whole runs; and the summed PSS of
  the process tree for anything that forks (kiln `KILN_WORKERS`, node `cluster`, bun
  `reusePort`), because a sum of RSS counts the shared binary text once per process.

The kiln interpreter is not measured here: its realm clock is virtual (ADR 0044) and every row
below is about the native runtime.

## The workloads

| # | workload | node / bun | Go | kiln today |
| --- | --- | --- | --- | --- |
| 1 | **Worker and actor start**: start a worker and get one message back; memory per added worker with 1, 10, 100 alive | `worker_threads` `Worker` (`js/start.mjs`) | goroutine + channel | `worker_threads` over a realm (`js/start.mjs`), plus `spawnActor` to ready and first call (`js/actor-start.mjs`); `KILN_WORKERS=N` processes are shown separately for deployment context |
| 2 | **Messages**: ping-pong round trip and one-way throughput of a short object; a ~1 MB object graph; a 16 MB `ArrayBuffer` copied vs transferred | across a `Worker` (`js/messages.mjs`), and in one realm over `MessageChannel` (`js/inrealm.mjs`) | channels; the graph deep-copied (the structured-clone equivalent) and by reference | the same cross-realm Worker and in-realm programs as node and bun |
| 3 | **Shared read-only dataset**: a 50 MB `Float64Array` lookup table, N workers each doing 2M pseudo-random lookups | copied to each worker, or one `SharedArrayBuffer` (`js/shared.mjs`) | one slice every goroutine reads | the same copied and shared-buffer program; a user object graph in the immutable region awaits issue 11 |
| 4 | **Many tasks**: 10, 1,000, 100,000 concurrent tasks, each waking 10 times on a 10 ms timer | async functions + `setTimeout` in one realm (`js/tasks.mjs`) | goroutines + `time.Sleep` | async tasks **in one realm**, labelled (a realm per task comes with issues 05 and 09) |
| 5 | **Data-parallel map** over 10M numbers: a compute-heavy callback, and a light one | `Array.prototype.map`; a `worker_threads` pool of one worker per core that copies each slice in and transfers it back (`js/pmap-pool.mjs`) | a loop; goroutines over sub-slices | `map`, and `parallelMap` (ADR 0058) natively, plus `parallelMap` with `KILN_PARALLELISM=1` to show whether the parallel lowering ran |
| 6 | **Server at all cores**: hello-world HTTP under `oha` | node `cluster` (`js/cluster.mjs`); bun `Bun.serve({ reusePort })` in one process per core (`js/bun-serve.mjs`) | `net/http` | `KILN_WORKERS=auto` in process and thread mode (`js/serve.mjs`); `KILN_WORKERS=1` for scale |
| 7 | **Separate scheduled realms**: 10, 1,000 and (when memory allows) 100,000 realms each waking ten times | The one-realm async task comparison is workload 4; isolate workers have a different memory budget | Goroutine comparison is workload 4 | `KILN_REALMS=N` with `KILN_REALM_SCHEDULER=1` (`js/realm-periodic.mjs`); every realm's output and status checked |

Details that matter when reading the numbers:

- **1.** The node/bun start is measured from `new Worker` to the first `message` event; each worker
  is terminated before the next starts, so it is start cost, not pool reuse. "Memory per added
  worker" is the RSS delta over the process before any worker was started, divided by N. The kiln
  actor row separately measures `spawnActor` until its ready handshake, then the first method
  reply; it stops each actor outside those clocks. It does not claim a node/bun comparison for
  kiln's actor API. The kiln
  process rows are wall time from spawn to the exit of the whole tree (supervisor included when
  N > 1) and PSS of the held-alive tree; "per added worker" is `(PSS(N) - PSS(1)) / (N - 1)`, so
  it includes a share of the supervisor. The node and bun "process start" row times the same
  one-line program as a whole process, for context against `KILN_WORKERS=1`.
- **2.** The one-way case sends 100,000 messages across threads but 10,000 in one realm, because
  kiln's in-realm channel runs at thousands of messages per second and 100,000 would take minutes.
  The per-message rate is what is compared. The 16 MB rows are a round trip: copied both ways, or
  transferred both ways.
- **3.** Setup is from the first send until every worker holds the table, with the workers already
  started; the copy row pays one structured clone per worker, the `SharedArrayBuffer` row none.
  JavaScript can share bytes, never an object graph -- the gap the shared immutable region (08) is
  for.
- **4.** The floor is 100 ms (10 ticks of 10 ms); everything above it is scheduling. Peak RSS
  includes each runtime's baseline.
- **5.** The pool rows include copying the input into typed arrays: that is what a pool of isolates
  has to pay. kiln's `parallelMap` accepts only callbacks its typed pool can prove numeric (ADR
  0058); others fall back to the sequential lowering and the `KILN_PARALLELISM=1` row then matches
  the default one.
- **7.** This is an end-to-end native launcher measurement: wall time includes the process launch,
  realm setup and teardown, timer turns, and two result files per realm. The runner checks all
  output files and statuses before accepting a timing. Before trying 100,000 realms, it projects
  RSS from the 10 and 1,000 realm runs and skips the row if that would exceed 70% of available
  host or cgroup memory with a 1.5× margin. A skipped row is a capacity limit, not a passing
  100,000-realm result.
- **6.** `oha` runs on the same machine and competes for the same cores, so absolute rps is a
  property of this box; the columns are comparable with each other. Requests `oha` cancels at the
  deadline are not counted as errors.

## What "not available" means

It means the runtime has no way to express that workload yet, rather than a failed run. Workloads
1-3 use `worker_threads` in kiln, workload 6 has process and thread modes, and workload 7 uses
the opt-in turn scheduler. A user frozen-value graph has its own API but is not part of the
shared-byte table in workload 3. A failed build or run is reported as a failure, and a checksum
mismatch is excluded.

A kiln build that fails is reported as a failure with the compiler's first error, not as "not
available".

## Files

- `run.sh` -- entry point; `run.py` -- builds, runs, aggregates, renders.
- `js/` -- the node/bun/kiln programs; `js/common.mjs` holds the shared workload pieces (the small
  message, the graph, the table, the map callback) so every runtime does the same work.
- `go/` -- one Go program with a subcommand per workload, printing the same JSON lines.
- `results-<date>.md` -- one file per recorded baseline.
