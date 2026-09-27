# Parallel built-ins benchmarks

kiln's parallel family (`parallelMap`, `parallelFilter`, `parallelReduce`, `parallelSort`; ADRs
0068 and 0069) against the sequential method in node, bun and kiln, against the realistic
alternative a node user would write (a `worker_threads` pool), and against Go with goroutines.
One command reproduces the table:

```sh
bench/parallel/run.sh            # 3 interleaved rounds, about 7 minutes
bench/parallel/run.sh --quick    # 1 round, shorter budget, about 2 minutes
bench/parallel/run.sh --only map-heavy,sort-array
```

It builds a release `kiln` if there is none (`mbx build --release -p kiln-cli`), builds each kiln
program with `--node --profile native-release --explain-parallel` (three at a time) and the Go
program once, all outside every measurement, and writes `results-<date>.md` here with the machine
details, the load average and the compiler's per-site verdicts beside the numbers. The raw
records go to `target/bench-parallel/<timestamp>/raw.json`. It needs `node`, `bun` and `python3`
on `PATH`; Go comes from `PATH` when a working `go` is there and otherwise from
`mise exec go@latest`.

This is issue 11 of `.scratch/parallel-builtins/`. It is extended as each tier lands: a row under
"Waiting on tiers 2-4" becomes a measured row, and a "(partial)" cell loses the qualifier.

## How it measures

The rules are `CLAUDE.md`'s "Benchmarking: time the work, not the launcher":

- **Warm, inside the process.** Every JavaScript program hands its workload to
  `bench/harness.mjs`: one cold call reported apart, a batch calibrated to outlast 50 ms, then the
  median of batches until the budget (1,000 ms; 400 ms in `--quick`) is spent. `go/main.go` does
  the same in Go. Setup -- building the million-element inputs, starting the pool's workers,
  building the records -- is outside the timed function.
- **Interleaved rounds, medians.** Each round runs every job once, the order rotated per round; a
  cell is the median across rounds, `~` when the rounds spread by more than 25%. The load average
  before each job is recorded and summarised.
- **Answers are compared.** Each program computes a checksum of its last result *after* the clock
  stops (the harness's `checksum` option, added for this suite so an array result is not summed
  inside every call): `length:sum` for arrays, the sorted array's ends and a weighted sample for
  sorts, the value for folds. They are compared at twelve significant digits, against node's
  sequential method (else Go). A differing cell is excluded and named under "Divergences": a
  fast wrong answer reads exactly like a win. Twelve digits forgive the last-ulp differences
  between libm's, V8's and Go's `sin`/`cos`; the folds and pipelines are integer-valued, so a
  parallel reduction's different addition order is exact.
- **Identical inputs everywhere.** Every value comes from one integer hash (`js/common.mjs`,
  reproduced in `go/main.go`), so no data file is shipped and every runtime sorts, filters and
  sums the same million numbers.

The kiln interpreter is not measured: its realm clock is virtual (ADR 0044). Every kiln column is
the native backend.

## Columns

| column | what runs |
| --- | --- |
| node / bun `map`/… | the ordinary sequential method (`map`, `filter`, `reduce`, `sort`), `js/<workload>.mjs seq` |
| node `worker_threads` pool | `js/pool.mjs`: one worker per hardware thread, started before the clock; see below |
| Go goroutines | `go/main.go`: one goroutine per `GOMAXPROCS` (every hardware thread) over contiguous sub-slices |
| kiln `map`/… | the same `js/<workload>.mjs seq`, compiled |
| kiln `parallel*` @1, @4, @N | `js/<workload>.mjs par`: the parallel method, with `KILN_PARALLELISM` set to 1, 4 and every hardware thread |

The `@1` column is the pooled lowering on one thread: comparing it with `kiln map` separates what
the parallel lowering's own code buys (a typed worker in place of a generic call per element)
from what the extra cores buy.

The pool and Go are written the way a programmer who wanted the cores would write them, which is
not always the same work as the methods:

- The pool's inputs live in `SharedArrayBuffer`s built before the clock, so no call copies them to
  the workers. `sort-records` cannot share its objects either: the pool sorts an index
  permutation by the shared price column, and the main thread applies it to the records. Each call allocates its output (a fresh `SharedArrayBuffer` for map and sort;
  per-worker `Float64Array`s transferred back and concatenated for filter), as `map` does.
  Records as objects cannot be shared: each worker receives its slice once, at start-up, and
  keeps it.
- **The pipelines are one fused loop per worker/goroutine** (score, test, add), where the methods
  make three passes and two intermediate arrays. That is what hand-written parallel code does,
  and it is why those two columns lead the pipeline rows.
- Sort: each worker/goroutine sorts its run, then runs are merged pairwise, one merge per
  worker/goroutine per level. The pool's `sort-array` answers a `Float64Array`, not an array.

## Workloads

All over 1,000,000 elements. The callbacks are written inline, identically, in every file.

| workload | the code | kiln today |
| --- | --- | --- |
| `map-light` | `xs.map(x => x * 1.5 + 2)` on a `Float64Array` | parallel |
| `map-heavy` | `xs.map(x => Math.sqrt(x * x + 1) * Math.sin(x) + Math.cos(x * 0.5))` | parallel |
| `filter` | `xs.filter(x => Math.sin(x) * x > cut)`, `cut` a captured `const` | parallel (tier 1) |
| `reduce-sum` | `xs.reduce((acc, x) => acc + x, 0)`; parallel: `parallelReduce(f, (a, b) => a + b, 0)` | parallel |
| `reduce-fold` | the same with eight rounds of `y = (y * 31 + k) % 1000003` per element | parallel |
| `sort-typed` | `new Float64Array(xs).sort()` (copy inside the timing) | parallel (default numeric order) |
| `sort-array` | `xs.slice().sort((a, b) => a - b)` on a number array | parallel (proven difference comparator) |
| `sort-records` | `records.slice().sort((a, b) => a.price - b.price)` on 1M `{ id, price }` objects | parallel (tier 2: a key comparator; the pool sorts by key and never calls it) |
| `pipeline-column` | order amounts: `.map(x => Math.floor(x * fx * taxed * 100)).filter(c => c > cut).reduce(sum)` | all three stages parallel (tier 1) |
| `pipeline-records` | records as typed-array columns: `price.map((p, i) => Math.floor(p * qty[i] * (5 + rating[i])))`, then filter and sum | all three stages parallel (the map reads the columns: tier 2) |
| `pipeline-objects` | records as `{ price, qty, rating }` objects: `records.map(r => Math.floor(r.price * r.qty * (5 + r.rating)))`, then filter and sum | all three stages parallel (the map reads properties: tier 2) |
| `enrich-records` | `records.map(r => ({ ...r, total: r.price * r.qty }))` over 1M `{ price, qty, rating }` | parallel (tier 3: the new objects are built in worker arenas) |
| `flatmap-records` | `records.flatMap(r => r.qty > 4 ? [r.price, r.qty] : [])` | parallel (tier 3: the arrays are built in arenas, flattened on the realm thread) |
| `format-csv` | `records.map(r => `${r.id},${r.qty},${r.price}`)`: CSV lines | parallel (tier 3: number formatting and concatenation in arenas) |
| `parse-csv` | `lines.map(l => l.trim().split(",").map(Number))`: parse CSV lines | parallel (tier 4: pure calls -- `trim`, `split`, `Number` on the workers) |

The tier-3 and tier-4 rows have no `worker_threads` pool or Go twin yet (their cells read `--`): the
comparison that matters for them is kiln's parallel method against its own and node's `map`.

The results file prints `kiln build --explain-parallel` for every program, so a reader sees which
site ran parallel and, when one did not, the reason code.

## What tiers 2-4 will add

Today's lowering (tier 1) takes a callback that reads numbers -- its parameters and captured
constants -- and returns a number, calling only `Math`. The PRD's next tiers each move a
realistic shape from "sequential" to "parallel":

- **Tier 2, read the heap** (issues 06, 07; landed): callbacks that read arrays, typed arrays,
  object properties and strings. `pipeline-records`' and `pipeline-objects`' score stage lost
  "(partial)", and key comparators such as `(a, b) => a.price - b.price` for `parallelSort` became
  the `sort-records` row.
- **Tier 3, allocate in worker arenas** (08; landed): callbacks that return new objects, arrays
  and strings -- the `enrich-records`, `flatmap-records` and `format-csv` rows.
- **Tier 4, pure calls** (09; landed): callbacks that call the program's own pure functions and
  the built-ins marked pure (`split`, `trim`, `Number`, `toFixed`, `JSON.stringify`, ...), which
  text processing needs -- the `parse-csv` row.

The results file's "Waiting on tiers 2-4" table lists the realistic workloads that need them;
each becomes a measured row, with every column, when its tier lands.

## Files

- `run.sh` -- entry point; `run.py` -- builds, runs, aggregates, renders.
- `js/common.mjs` -- inputs, arguments, checksums; `js/<workload>.mjs` -- one program per workload,
  `seq` or `par` (node and bun only run `seq`).
- `js/pool.mjs`, `js/pool-worker.mjs` -- the `worker_threads` pool.
- `go/` -- one Go program with a subcommand per workload.
- `results-<date>.md` -- one file per recorded baseline.
- `fallback/run.sh [kiln]` -- each parallel method with its pool declined against its sequential
  method (parallel issue 15), per array kind: the two columns should be within noise. One small
  program per method (`fallback/<method>.mjs`), so each stays under the release profile's
  specialization limits.
