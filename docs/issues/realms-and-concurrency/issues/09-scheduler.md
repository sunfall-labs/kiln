# 09. The scheduler: turn migration, pinning, deterministic mode, one pool

Status: resolved (concurrent actor stop race fixed; program gate and collector stress passed)

The actor extension has an explicit async-only wait contract (ADR 0072): actors share scheduled
lanes by default, while ordinary Node-compatible workers retain dedicated threads. ADR 0070's
2026-09-28 amendment makes that exception part of the final scheduler contract. Synchronous
Node worker waits keep their live stack; scheduled actors use `Atomics.waitAsync` or messages.

ADR 0070's scheduler:

1. **One bounded pool for eligible turns.** Scheduler threads, one per core, run async-capable
   realm turns and ADR 0069's parallel chunks from one work-stealing queue. The separate Rayon
   pool in the provider is removed. Tokio stays the reactor and wakes realms.
2. **Turn migration.** A realm's next turn may run on any scheduler thread. The mid-turn collector
   records the stack base at the start of each turn, not once per realm.
3. **Pinning.** A realm with an addon, FFI callback or lane-affine I/O keeps its owner lane.
   Node workers that may call synchronous `Atomics.wait` retain dedicated threads.
4. **Blocking hand-off.** A blocked scheduled lane hands movable work to a replacement; its
   live stack and pinned work remain on that lane. This does not promise progress for arbitrary
   synchronous worker graphs on a fixed pool.
5. **Deterministic mode.** Every realm on one thread under the virtual clock, for tests.

A human reviews the design first: it is the runtime's largest change since ADR 0028.

## Acceptance

- 10,000 idle realms in one process, and a benchmark with one realm per connection, run with the
  thread count at cores plus the reactor.
- The 05 harness passes with migration on, and identically in deterministic mode.

## Comments

### 2026-09-29 -- scheduled worker exits read from their saved realm scope

The 16-actor stop failure was a false worker error, not a lost scheduler wake. A scheduled
turn saves its `EXIT_REQUEST` in `NativeRealm.worker_scope` before releasing the lane. The
generated launcher then called `kiln_worker_exit_code` outside `ActiveRealm`, reading the
lane's empty thread-local slot instead. It reported a normal actor termination as worker
error `undefined`, rejecting `Promise.all(owner.stop())` and sometimes ending the parent
with unsettled top-level await. The launcher now enters the finishing realm to read its
saved exit request, for both workers and top-level `process.exit` in thread mode. Its
realm-entry failure remains an error rather than silently becoming a normal exit.

`conformance/native/actors-concurrent-stop.mjs` holds one shared graph in 16 actors,
queries them concurrently, and stops them concurrently. Its native-release benchmark
equivalent completed 20/20 repeated runs with the default scheduler. The `kiln-native`
crate passed 241 tests (2 ignored), the native corpus reported 352 matched, 1 known
refusal and 0 divergences, and the new probe matched with 1, 2 and 8 simultaneous
top-level realms. The actor core also takes the shared-value predicate from the
bootstrap's private structured-clone bridge; this preserves `kiln:actors`' own
disabled-extension diagnostic without importing `kiln:shared` first.

### 2026-09-29 -- concurrent scheduled actor stops can strand top-level await

The public shared-object benchmark starts 16 actors, loads and queries the same frozen graph,
then calls `Promise.all(owners.map(owner => owner.stop()))`. The load and query complete with
the expected checksum. Intermittently, the top-level await exits with status 13 or an unowned
function-body error during the stop phase. Temporary diagnostics showed all 16 stop messages
sent, but only 11 workers logged receipt/completion before one failing exit. In a separate
failing run, 11 `stopped` replies arrived and only 10 worker exits were observed. Six comparable
runs with `KILN_REALM_SCHEDULER=0` completed all 16 stops and exits. More instrumentation in
the worker made 12 scheduled runs pass, so this is timing-sensitive; the dedicated-thread
comparison narrows it to the scheduled path but does not prove which wake is lost. Selected
failure and success logs, with the quick benchmark report, are under
`~/.cache/kiln/actor-shared-2026-09-29/`. Restore this issue to resolved only after a focused
repeated native-release probe
passes with the default scheduler and the ordinary actor shutdown tests remain green. A timer
retry would conceal the wake/liveness fault and is not the intended fix.

### 2026-09-28 -- accept the bounded async scheduler and the dedicated Node exception

ADR 0070 now states the exception required by the two-parent synchronous-wait counterexample
below: no finite fixed-lane routing rule guarantees progress when waiting parents occupy lanes
that own pinned notifier children. Actors reject synchronous waits, so their turns and parallel
chunks share the bounded pool; Node-compatible workers keep dedicated threads and full wait
semantics. A future continuation is a separate compiler, collector and runtime project with the
proof obligations in `worker-wait-continuation.md`.

The scheduled launcher completed 10,000 idle realms with 18 peak threads on 16 cores, and the
full migrated native-realms sweep matched 325 probes with seven declared skips and zero
divergences. Deterministic timer, port, blocking-wait and HTTP probes matched at 1, 2 and 16
realms; the live TCP callback-lane test passed. The one-realm-per-connection benchmark completed
500 connections with 18 peak threads; bounded successor preference brought server-observed
response p95 from 108 ms to 3.88 ms, with every checksum intact. These results establish this
issue's async-scheduler acceptance. The 100,000-realm memory/capacity target remains in issues
08 and 13, and the final program gate remains due.

### 2026-09-28 -- bounded successor-turn preference removes the connection tail

The shared ready queue now separates first turns from successors. A realm resuming after an
external wake or an immediately ready turn receives priority, but each lane selects a first
turn after at most fifteen successors when both are queued. FIFO order within each class,
the pinned/movable alternation, ticket generations and cancellation remain intact. This is a
bounded admission rule, not an unbounded I/O priority.

Three interleaved native-release connection rounds on the same host measured the 500-realm
scheduled median at **121 ms wall and 3.88 ms server-observed response p95**, with 18 peak
threads. The prior FIFO run at `8bda95ef` was 208 ms and 108 ms p95; 100 realms changed from
47.9 ms and 25.2 ms p95 to 31.7 ms and 3.45 ms p95. A quota of 3 successors left the 500-realm
p95 at 51.8 ms, and 7 left 9.37 ms; 15 was the smallest measured quota near pinned-realm
latency. Raw records are in
`~/.cache/kiln/realm-bench-2026-09-28/wake-priority-connections.json`.

All 211 runtime tests passed, including queue admission and wake-race fixtures. A targeted
native-realms run at 1, 2 and 16 realms matched six timer, actor, message, worker and parallel
probes with no divergence. The routine gate passed all 10 steps in 657 seconds: 2,272 crate
tests passed, native parity had 341 matches and zero divergences, and routine Test262 had no
chunk regressions. The gate aggregate and raw logs are preserved under
`~/.cache/kiln/realm-bench-2026-09-28/wake-gate/`.

### 2026-09-28 -- connection latency after scheduled actors

The three-round native-release connection probe at `8bda95ef` completed every checksum. At 500
connections, scheduled realms had a 208 ms median process wall time with 18 peak threads;
pinned realms took 215 ms with about 487 peak threads. Server-observed response p95 remained
108 ms scheduled against 2.46 ms pinned. At 100 connections, the corresponding p95s were 25.2
and 0.97 ms. The server's last accept preceded its last response by about 16 ms in the 500-realm
scheduled rounds, while early accepts waited much longer. The ready queue is FIFO within its
pinned and movable classes; wakeups from parked I/O realms plausibly sit behind the initial
500-turn admission burst. This is an inference to test with queue instrumentation or a weighted
wakeup lane, not yet an established cause or a latency fix. Raw records are in the preserved
`~/.cache/kiln/realm-bench-2026-09-28/realm-connections.json`.

### 2026-09-27 -- design review

The project owner authorized implementation of `scheduler-design.md` after review. The staged
plan was tightened so all realms first run pinned on scheduler lanes; migration starts only after
the provider registry, collector stack-base rule, retained pointer paths and pin transitions
have been audited. No scheduler implementation is claimed by this note.

### 2026-09-27 -- reactor and lane-local task split

`kiln-runtime` now separates the Tokio reactor from a lane-local `LocalSet`. A shared wall-clock
reactor has one I/O worker and can be entered concurrently by scheduler lanes; a deterministic
lane keeps its own current-thread reactor and paused clock. A focused test ran local tasks on two
OS lanes over the same reactor and checked their thread identities. The existing current-thread
constructor and 169 runtime tests still pass; provider and CLI dependency checks pass. Realm
turns still use the old pinned launcher, pending the scheduler tickets and affinity audit.

### 2026-09-27 -- exclusive turn tickets

`kiln-runtime::turn_queue` now provides the scheduler's generation-checked ready queue and
exclusive turn lease. Repeated wakes coalesce; a wake during a running turn is retained for
one successor turn; cancellation invalidates a queued generation. A dropped unfinished lease
retires its ticket. Four focused tests cover duplicate wakes, stale entries, multi-lane claims
and a wake racing turn completion. The queue holds only IDs and tickets; the native launcher
is not connected yet, and no realm state has been made cross-thread.

### 2026-09-27 -- local-task wake bridge

`Executor::poll_local_once` now polls a realm's Tokio `LocalSet` without parking a scheduler
lane. It registers the caller's ticket waker, so a local I/O task made ready later can queue that
realm again. A focused test signals a local task from outside the lane and verifies that the
scheduler waker fires and the task resumes on its original OS thread. This is the missing wake
bridge for multiplexing several parked, pinned realms on one lane; the native drive is not yet
using it.

### 2026-09-27 -- fixed lanes with pinned realm bodies

`PinnedTurnScheduler` now starts a fixed number of lanes over one reactor. A `Send` factory
constructs each `!Send` body on its assigned lane; only tickets and commands cross threads.
The lane executes one turn per lease and drops the body there after completion or cancellation.
Three focused tests cover 200 realms sharing two lanes, owner-lane teardown, and a panicking
realm leaving its lane usable. Native realms still use the old launcher; connecting its
one-turn entry point and parking/wake source is the next step.

The lane bridge also passed a focused timer test with 100 parked realm bodies over two lanes:
each realm's own `LocalSet` timer woke its ticket on the shared reactor, resumed on the same
lane, and completed. This establishes the nonblocking pinned path before connecting generated C.

### 2026-09-27 -- generated launcher lifecycle

The graph launcher now keeps its graph and result slots on the heap and makes its advance state
explicit. It advances one module body or one provider drive per call. A settling module still
drives once and then enters its body, preserving the previous order. Initialization and advance
are distinct C functions;
the existing wrapper calls them continuously and still owns teardown. The provider drive still
waits to quiescence, so this alone does not reduce native realm threads. The next provider step
must return after one language turn and let the scheduler own wake/deadline registration; the
session record must move off the wrapper's stack before it is scheduled. The session record is
now a heap allocation with explicit cleanup on normal, worker and failed-begin exits; the
continuous wrapper still owns it until the provider exposes the one-turn drive.

The scheduler also has a deterministic constructor: one FIFO lane with a separate pausable
current-thread reactor. A focused test confirms that twenty realm bodies all execute on that
lane and can pause its clock; the wall-clock lanes retain their shared reactor. The generated
launcher has not yet selected this mode.

Pure timer deadlines now use one min-heap per scheduler lane. A parked realm contributes only a
deadline and a generation, and an external wake invalidates its old deadline when its next turn
runs. No timer task or thread is allocated per realm. A focused test parked 100 realms over two
lanes and confirmed each resumed on its owning lane. The existing Tokio local-task wake test and
all four ticket-queue tests also pass. As a regression check for issue 24, the native realm
harness ran `timers.ts` 100 times each at 16 and 64 simultaneous realms: one probe matched,
zero diverged. This still exercises the older dedicated-thread launcher until the provider step
is connected.

### 2026-09-27 -- pinned native scheduler bridge

The provider now exports a one-turn realm drive and a scheduled launcher. The generated graph
keeps its session on the heap across turns, and the provider constructs it on a fixed scheduler
lane. An idle realm parks a ticket, not its OS thread; local Tokio tasks and pure timer deadlines
wake that ticket. The virtual-clock path uses one deterministic lane. The scheduler remains behind
`KILN_REALM_SCHEDULER=1` while the migration, affinity, blocking, and shared parallel-pool stages
are still open. The provider fixture ran 200 realms on two lanes. A generated-C top-level-await
fixture and the native realm harness passed for `timers.ts`, `timer-after-top-level-await.mjs`,
`message-port-transfer.mjs`, and `many-tasks-timers.js` at 1, 2, and 16 realms.

### 2026-09-27 -- 10,000 idle pinned realms

The scheduled launcher now accepts 10,000 realms; the older dedicated-thread launcher retains
its 1,024 cap. Scheduled harness output uses a memory stream until teardown, then writes each
realm's output file, so parked realms do not each hold an open file descriptor. A wall-clock
program with a ten-second idle timer completed with 10,000 output files and 10,000 zero statuses
in 12.75 seconds on a 16-core Linux host. Peak process counts were 18 threads (16 lanes, one
reactor, and the caller), 10 file descriptors, and 2,653,364 KiB RSS. This meets the idle thread
count acceptance, but the roughly 260 KiB per realm and the one-realm-per-connection benchmark
remain open. The fixed-lane path is still opt-in; migration and the shared parallel queue are
separate stages.

### 2026-09-27 -- collector stack bound follows the lane

The mid-turn collector formerly kept the stack bound in the realm heap, so a heap moved to a
different scheduler lane would scan an address range from its old thread. The bound is now
cached in thread-local storage and queried once on each OS thread; the heap carries none. A
focused C fixture collects a heap on one thread, moves the live root to another, collects there,
and checks both the distinct stack bounds and the preserved bytes. It passes, as do all 36
emitted-C snapshot tests and two native realm probes at 1, 2, and 8 realms with GC stress. The
ordinary native corpus passed 318 matched, 1 refused (`shadow-realm`), and 0 diverged. This
change does not yet enable realm migration; the full native-stress gate remains due at the end
of the program.

### 2026-09-27 -- shared ready queue and movable turn body

The fixed lanes now consume one ready queue with lane-local pinned entries and a shared movable
entry list. Each pinned wake notifies its owner; a movable wake notifies one idle lane. The
queue alternates its choice when both kinds are ready, so a stream of pinned turns cannot starve
movable work. A `Send` turn body is stored outside any lane and is taken only under its exclusive
generation-checked ticket lease; it is put back after the turn has returned. A focused test
blocks each lane in turn and confirms the same body runs successive turns on different OS
threads. The six queue tests and seven scheduler tests pass. Native generated bodies remain
pinned while the provider's `Rc`, local-task and FFI affinity audit continues.

The next scheduler slice lets a factory create a `Send` body on a lane and bind its ticket to that
lane before first publication. A running body can also bind its ticket to its current lane;
the generation lease retains a racing wake and routes its successor after the binding. That is
the one-way transition the native adapter needs when initialization or a later turn starts an
executor-local task, attaches HTTP, or publishes a foreign pointer. It does not yet move a native
realm; the runtime queue and factory tests pass.

A body that pins during a turn is promoted out of the shared `Send` registry and into its
current lane's private body map before its successor can run. Cancellation and destruction then
occur on that lane. A focused test pins on the first turn, parks on the second, cancels from the
caller, and asserts that destruction ran on the pinned OS thread. This closes the scheduler-side
destructor hazard before the native provider starts using movable tickets.

### 2026-09-27 -- opt-in native turn migration

The generated launcher can now use `KILN_REALM_MIGRATE=1` together with
`KILN_REALM_SCHEDULER=1`. A scheduled session holds the sole realm registry reference between
turns, drops its empty Tokio `LocalSet` on the source lane, and installs a fresh one on the next
lane. The ticket stays exclusive throughout. The provider pins before creating a local task,
submitting an asynchronous host operation, attaching HTTP, using FFI state, or entering
`Atomics.wait`; a newly pinned body moves into the owner's lane-local map before another turn
can run or cancellation can destroy it. Virtual-clock realms stay on the deterministic lane.

The lane transition clears emitted thread-local frame registers through a distinct GC root
callback phase. The ordinary teardown phase can release fallback key ownership, so it must not
run at a migration boundary. A provider test checks the two lane-reset calls across a handoff
and 100 three-turn sessions verify that an unpinned realm actually runs on both lanes. The
generated wall-clock timer program completed 16 realms with 16 zero statuses. This path remains
opt-in until the blocking replacement lanes, shared parallel queue, and full stress acceptance
are complete.

### 2026-09-27 -- parallel map chunks share the realm lanes

The provider now hands parallel map chunks to the active realm scheduler instead of starting a
private Rayon map pool. A call made outside a scheduled realm uses a lazily created scheduler pool;
the calling thread also claims chunks, so simultaneous calls from all lanes still make progress.
The measured prefix and chunk cost probe are unchanged. Cancellation releases queued bodies
before the join returns to C, and a focused test covers cancelling a body during its turn.
All 47 provider tests with the parallel feature and the focused runtime cancellation test pass.
Parallel sort still uses Rayon; its stable merge plan, blocking replacement lanes, and the full
acceptance runs remain open.

### 2026-09-27 -- stable sort uses the scheduler queue

Parallel sort now sorts disjoint runs and merges adjacent pairs through the same movable-ticket
queue as map chunks. Equal elements come from the left run first, preserving stable order; the
final buffer is copied back only when a merge pass ends in scratch storage. The provider no
longer depends on Rayon. A focused two-lane test compared 30,017 items with many ties to Rust's
stable sort, and the provider's numeric, signed-zero and NaN-order tests pass. Blocking
replacement lanes and the full 10,000-realm/connection acceptance remain open.

The generated launcher now reads `KILN_PARALLELISM` as the shared lane budget, including the
one-lane sequential setting. Without an explicit budget, `KILN_SCHEDULER_LANES` can override the
default; otherwise each supervised process worker takes its share of online CPUs from
`KILN_WORKER_COUNT`. The provider caps its parallel method count to the active scheduler budget.

### 2026-09-27 -- bounded blocking replacements

A scheduled wall-clock turn entering `Atomics.wait` or a native FFI call now starts a temporary
replacement lane before it blocks. The queue reserves lane identities up front, so a movable
realm that pins on a replacement keeps its thread-affine body there; when the blocker returns,
the replacement drains after its pinned bodies finish. Pure timer deadlines move back to a base
lane during that drain. `KILN_SCHEDULER_REPLACEMENTS` bounds simultaneous replacements and an
exhausted limit returns an error. Deterministic virtual-clock mode retains its one lane and
allows a blocking wait for an external worker without creating a replacement.

Thirteen focused scheduler tests pass, including bounded replacement, deadline handoff and
pinning on the replacement. The 54 provider tests with FFI enabled pass. A new native
`Atomics.wait` probe matched alone and at 1, 2 and 4 deterministic realms; its shipped
wall-clock binary completed at 1 and 4 realms on one base lane. The probe also caught and
fixed a scheduled-drive `RefCell` reborrow when a parked wait had not woken yet. A dedicated
FFI/addon/HTTP lane-identity acceptance probe and the connection benchmark remain open.

### 2026-09-27 -- idle capacity and connection workload

The native-release `hold` program with 10,000 scheduled, migratable realms completed in 2.92 s:
10,000 zero statuses and `hold` outputs, 18 peak process threads on a 16-core host (16 scheduler
lanes, one reactor, one caller), and 2,549,264 KiB peak RSS. The thread bound still holds after
the shared parallel queue and blocking-replacement work. Issue 08 owns the remaining ~263 KiB
anonymous-memory slope per realm.

`bench/concurrency/realm-connections.py` now reproduces a one-realm-per-TCP-connection comparison
with the same loopback server and checksum for every runtime. A quick, single-round run at 16
lanes and 100 simultaneous connections measured 47 ms / 18 threads / 76.7 MB peak RSS for the
scheduled launcher, 53 ms / 101 threads / 66.8 MB for the prior pinned launcher, and 487 ms /
107 threads / 825 MB for Node worker threads. These are process wall times, including creation,
connection, response and teardown; they are a smoke measurement on a shared host, not a quiet
throughput baseline. At 500 connections the scheduled launcher was substantially slower than
the pinned launcher in preliminary runs (~1.2 s versus ~0.2 s); investigate the queueing cost
and rerun interleaved rounds before treating the scheduler performance as settled. The full
migration/native-realms and explicit addon/FFI/HTTP lane-affinity checks remain open.

### 2026-09-27 -- virtual HTTP turns advance their reactor

The 05 harness found a deterministic-mode stall at `app-http-client-url`: a one-lane virtual
realm accepted the socket and dispatched the request, then parked after its handler armed a 1 ms
timer. A current-thread Tokio reactor has no background I/O driver, so each scheduled poll now
gives it one bounded runtime tick. A virtual waiter also reports when its timer has actually
been polled after `yield_to_tasks`; only then does the provider advance paused Tokio time. Tokio
rounds timer deadlines to its next millisecond tick, so the provider advances that private clock
one extra millisecond while the language clock still runs the timer at its declared deadline.
A focused runtime test reproduced the exact-tick failure and passes with the cushion.

With migration enabled, `scripts/native-realms.sh` matched all four selected probes at 1, 2 and
16 realms: `timers.ts`, `message-port-transfer.mjs`, `scheduler-blocking-wait.cjs` and
`app-http-client-url.mjs`. This covers deterministic timer, port, blocking-wait and HTTP request
progress. A full migrated corpus, explicit FFI/addon/HTTP OS-lane identity assertions and the
final stress gate remain open.

Three interleaved native-release rounds of the connection probe at 16 scheduler lanes and 100
connections gave median process times of 1,396 ms for Node workers, 100 ms for pinned kiln realms,
and 125 ms for scheduled kiln realms. At 500 connections, the pinned median was 515 ms and the
scheduled median 1,523 ms; both produced 500 correct replies. The scheduled process used 18 peak
threads versus about 493 pinned threads. A separate same-server run showed its last response at
1,373 ms of 1,422 ms wall time, versus 319 ms of 347 ms for pinned: the gap lies in serving
connections, not teardown. At 500, changing the lane count from 16 to 32 barely improved the
scheduled time, and the shared reactor's Tokio worker consumed less than one clock tick while
the lanes accumulated roughly two seconds of CPU in total. This does not yet identify the
contention point. Keep scheduling opt-in until its 500-connection path and full migrated corpus
have been investigated; the benchmark now records server-observed timing to make that work
reproducible.

The 500-connection bottleneck above was the benchmark server's default listen backlog of 100,
not the scheduler. With all other settings fixed, explicitly restoring backlog 100 reproduced
1,243 ms scheduled wall time and a 1,217 ms last accept. At backlog 4,096, three interleaved
500-connection rounds had medians of 235 ms scheduled and 245 ms pinned, all 500 checksums
correct. Scheduled used 18 peak threads; pinned peaked at 442-490. The no-I/O 500-realm
console probe likewise took 118 ms scheduled and 131 ms pinned. The scheduled HTTP responses
still had a higher server-observed p95 (115-130 ms versus 2-3 ms), reflecting the burst and
queueing pattern; the aggregate connection acceptance is met, but latency distribution deserves
separate tuning. The full migrated native-realms corpus and explicit addon/FFI/HTTP OS-lane
identity checks still gate making the scheduler the default.

### 2026-09-28 -- affinity boundary and worker scheduling audit

A focused provider test calls the HTTP attachment ABI from a wall-clock realm with an invalid
transport and verifies the HTTP pin is set before the handoff attempt. That realm cannot be
detached for migration afterward. The test passed with the `realm-http` feature; it does not
exercise a live socket or prove callback OS-lane identity. FFI has a separate live-library
identity test; addon loading has no implementation to exercise yet.

`node:worker_threads` and module-addressed actors still launch their whole graph through
`WorkerPool::spawn_pinned_with_stack`, one OS thread per live worker. The fixed-lane scheduler
only hosts top-level native realm sessions and parallel chunks today. Workers need a one-turn
graph entry and a wake source before their execution can safely move onto the shared queue.
The default remains the dedicated path while the full migrated corpus, live HTTP affinity,
and worker integration remain open.

### 2026-09-28 -- worker state follows scheduled top-level turns

A scheduled top-level realm now carries its worker protocol state across lane turns: its
broadcast-program identity, child worker keys, pending `process.exit` request and exit ownership
are installed only while that realm holds the exclusive turn lease. Teardown ends only that
realm's children. This closes cross-program `BroadcastChannel` routing and child/exit-state mixing
when several scheduled realms share one OS lane; the old thread-local behavior remains for a
worker's dedicated runner. Runtime and native-provider tests alternated two realms on one lane
and checked both broadcast isolation and preserved exit state. `node:worker_threads` and actors
still launch their whole graph on dedicated threads; their turn-based graph entry, wake routing
and per-turn worker context are the remaining worker scheduling integration.

The worker protocol now has a per-realm turn scope for worker identity, termination, signal
program, children and exit state. A native realm captures the worker and signal identity when it
is created, and each scheduled turn installs only that realm's values. The dedicated worker runner
uses the same scope for its continuous graph run, removing the separate unsafe borrowed
termination flag. A focused runtime test alternated a worker and an unrelated realm on one lane
and checked identity, termination, signal ownership and exit state. This is a prerequisite for a
turn-based worker graph; the worker launcher still uses a dedicated thread.

The first full migrated native-realms pass found `errors.ts` and `deep-recursion.js` crashing at
one realm and the release recursion probe overflowing its stack. Scheduler lanes had Rust's
default ~2 MiB stack while dedicated realm threads had 64 MiB. The base and replacement lane
builders now use that same 64 MiB budget. The provider source identity also needed to include
`turn_scheduler.rs`; otherwise native binaries could link a cached staticlib without the lane
fix. With both changes, the three affected probes matched at 1, 2 and 16 migrated realms.

The generated worker launcher now has heap-owned begin, one-step advance, identify and finish
functions. Its current dedicated runner calls these functions continuously, and the native
declared-worker fixture passed. The provider still invokes the dedicated runner; worker tickets,
wakes and completion ownership have not yet been connected to the scheduler.

The runtime now accepts a one-turn worker body on a scheduler ticket. It installs the worker's
protocol and signal scope for construction, every advance and finish; cancellation wakes parked
workers and completes them on their lane before they can park again. A completion guard also
reports one exit and releases the running count if the scheduler discards the factory before its
first turn. Parent teardown cancels scheduled children without joining from the same lane. The
runtime's fourteen focused worker tests pass, including a worker that always parks and a
factory cancelled before installation. The native provider has not yet selected this path.

The native provider now registers the worker graph's one-turn callbacks and selects the shared
scheduler for a worker when `KILN_WORKER_SCHEDULER=1` and its parent is itself on a scheduled
realm turn. The worker stays pinned to one scheduler lane; the provider installs that lane's
reactor during begin and its ticket waker during each advance. The same scheduler queue also
serves parent realm turns and parallel chunks. The dedicated worker path remains the fallback
for programs without an active realm scheduler. Provider/native checks passed, and the native
realm harness matched `worker-threads-plain-graph.cjs` and `actors-module-addressed.mjs` at
1 and 2 migrated realms with scheduled workers enabled. Default enablement, a larger worker
stress run, and auditing worker migration remain open.

The worker affinity audit keeps deterministic virtual-clock realms on the dedicated worker path:
one scheduler lane can block in `Atomics.wait` while a child must run to notify it. With the
opt-in worker switch set, `scheduler-blocking-wait.cjs` and
`worker-threads-shared-memory.cjs` matched at 1 and 2 deterministic realms after that fallback.
A shipped wall-clock worker graph did use the shared queue: 16 migrated top-level realms with
`KILN_PARALLELISM=2` produced 16 matching statuses and outputs, with four peak process threads
(two scheduler lanes, one reactor, one caller). The worker stays on its assigned lane through
local Tokio/HTTP work, FFI and `Atomics.wait`, so those thread-affine paths cannot migrate.

Worker scheduling remains opt-in. A wall-clock parent can synchronously block its lane in
`Atomics.wait` while a pinned child ticket is assigned to that same lane; a replacement lane
cannot run a pinned child. Several parents can also block different lanes in a cycle. The
runtime needs a nonblocking wait continuation or a routing proof that a notifier can run before
scheduled workers may become the default. Top-level scheduler default still awaits the full
migrated corpus and live HTTP affinity acceptance.

### 2026-09-28 -- migrated corpus found a stream capture gap

A full `KILN_REALM_SCHEDULER=1 KILN_REALM_MIGRATE=1` native-realms pass at 1, 2 and 16 realms
recorded 322 matches, 8 expected skips and 2 divergences. In `node-console-members.cjs`, a
`Console(process.stdout, process.stderr)` instance lost its final two lines under the scheduled
launcher, even at one realm. The launcher captures output with a `fopencookie`/`funopen` `FILE *`
that has no file descriptor; the launcher called `fileno`, and the Node stream wrote to the
process's stdout instead of the realm's capture. Node stream writes now use the current realm's
host `FILE *`, preserving their order with ordinary console output and preserving distinct
worker stdout/stderr streams.

The second divergence was a test race: `worker-threads-broadcast.cjs` printed after its two
worker replies without waiting for the main realm's own listener. With 16 realms, that listener
could run last. The probe now waits for all three deliveries before printing. Node returned the
expected three lines, and both changed probes passed native interpreter parity (2 matches) and
the migrated native-realms harness at 1, 2 and 16 realms (2 matches). The full migrated corpus
must be rerun after integration; a live HTTP callback-lane check remains unavailable in the
current socket-restricted environment. Scheduler default stays opt-in.

### 2026-09-28 -- why worker lane routing cannot enable the default

The proposed bounded dispatch rule, "put a worker on a lane other than its
currently running parent," is insufficient even with blocking replacement
lanes. Consider two scheduled parents on lanes A and B. Parent A waits on a
cell that its child on B will notify; parent B waits on a cell that its child
on A will notify. Both `Atomics.wait` calls retain their generated C stacks
and synchronously occupy A and B. The children are pinned tickets: a
replacement lane can run movable chunks and realms, but cannot acquire
either child's lane-owned worker session. Each parent therefore needs a
child turn that cannot start until a parent times out. Infinite waits do not
recover. The focused
`two_waiting_parents_starve_cross_routed_pinned_notifiers` runtime test uses
the actual waiter list, two pinned lanes and cross-routed notifier tickets;
its finite timeout makes the cycle observable without hanging the test.

This is a counterexample to any finite fixed-lane routing rule for arbitrary
worker graphs: every lane can hold a synchronous waiter whose notifier is a
pinned ticket on an occupied lane. Adding bounded replacement lanes does not
change the ownership of pinned tickets. Moving a child only at a turn
boundary would help an eligible movable child, but cannot guarantee progress
for a child with local Tokio tasks, HTTP, FFI, addon or other lane affinity.
The `WAIT` primitive has a synchronous return through `arrays.c` into
generated JS frames, so yielding a pending code would discard the continuation
required to return `"ok"`, `"not-equal"` or `"timed-out"` at the call site.

Keep scheduled native workers opt-in and dedicated workers as the default.
The default can change after a stackful, GC-visible continuation or an
equivalent compiler transformation preserves synchronous JS call semantics,
and a test proves notifier progress with all base lanes waiting, nested calls,
termination, timeout and pinned children. Reserving one OS lane per potentially
blocking worker also provides progress, but is the existing dedicated-worker
model rather than the bounded-thread scheduler acceptance.

The [worker wait continuation investigation](../worker-wait-continuation.md)
traces the generated C, Rust executor and GC stacks that a synchronous wait
would have to preserve. It records the fiber context, reclamation and
termination proofs required before this opt-in worker path can become the
default.

### 2026-09-28 -- full migrated native realm sweep after integration

With `KILN_REALM_SCHEDULER=1`, `KILN_REALM_MIGRATE=1`, and
`KILN_REALMS_COUNTS="1 2 16"`, the complete `scripts/native-realms.sh` corpus on the
integrated branch reported **325 matched, 7 skipped, 0 diverged**. The seven skips were the
existing `ShadowRealm` native refusal, three probes that call process-wide `process.exit`,
and three probes that write fixed filesystem paths. The ordinary-mode
`kiln-concurrency-engine-method` probe now builds and matches; the stream and broadcast
probes that diverged in the preceding sweep also match at every realm count. This clears the
full corpus item from the opt-in scheduler's migration acceptance. A live HTTP callback-lane
identity check and the scheduled-worker wait liveness condition above remain open.

### 2026-09-28 -- actor mailbox wake without a blocking lane

The actor worker already receives ordinary calls through `MessagePort` events;
an empty inbox uses the runtime's `PortWait` wake and parks its realm turn.
Graph receipt had one exception: after a placeholder arrived before native
publication, it retried `GRAPH_RECEIVE` with `setTimeout(0)` until the root
appeared. The publication lease now posts one private wake to the actor's
existing inbox after commit or abort. The actor retains the pending request
without a timer and rechecks `GRAPH_RECEIVE` on that wake. The number in the
wake is advisory; the host still authenticates the reservation and the native
inbox root, so a forged message cannot grant a graph. A wake racing the first
check is retained by the inbox queue, and a close or aborted publication makes
the pending request fail closed.

The focused runtime test confirms that both successful and abandoned leases
wake `PortWait`; the native actor reservation and module-addressed probes
complete. This makes the actor mailbox wait nonblocking and removes its busy
poll. It does not make arbitrary actor methods safe for scheduled workers:
they may call synchronous `Atomics.wait` through nested code. The native actor
worker therefore keeps the dedicated default until the continuation and
notifier-progress gates in `worker-wait-continuation.md` pass.

### 2026-09-28 -- migrated top-level realms by default

`KILN_REALMS` now selects the shared turn scheduler and turn migration by default. The full
325-match migrated sweep above establishes native-probe parity for this path; a focused run of
timers, declared worker graphs and module-addressed actors matched at 1, 2 and 8 realms without
scheduler flags. `KILN_REALM_MIGRATE=0` retains fixed scheduler lanes and
`KILN_REALM_SCHEDULER=0` retains the older dedicated-thread launcher (the timer probe matched at
1 and 2 realms with the latter). This switch affects the top-level `KILN_REALMS` launcher only;
native actor workers still default to dedicated threads until the synchronous wait continuation
has a liveness proof. A live HTTP callback-lane identity check remains outstanding because the
current test environment denies loopback socket creation.

### 2026-09-28 -- bounded controls on claimed realm turns

A pair claim can now queue a bounded, non-JavaScript control callback for either paused body.
Pinned bodies receive it on their owner lane; movable bodies receive the executing lane index and
must reinstall their realm-local context there. The command carries only numeric IDs across the
queue, and the callback runs only after both realms acknowledge the same claim. A callback panic
retires its body but defers its destruction until the claim releases it. If an unrelated turn
occupies the lane, an atomic pending/running handshake cancels a command that has not started
after one second; an already-running callback must finish before the claim can be released.
Three focused scheduler tests cover the successful path, panic teardown, and cancellation behind
an unrelated blocked turn. This is a scheduling seam for issue 10, not an ownership certificate.

### 2026-09-28 -- HTTP callback lane through the scheduler

A focused runtime test attaches a Hyper HTTP server connection to a pinned scheduler realm over
Tokio's in-memory duplex transport. A client on another OS thread sends a real HTTP request and
reads the response. The scheduler parks and wakes the realm through its local-task bridge, and
every HTTP event and checkpoint asserts that it runs on the original attachment lane. The test
passes without a loopback socket. It exercises the HTTP engine's callback and response path;
an actual socket callback-lane check remains due when the environment permits binding.

### 2026-09-28 -- actors use the bounded scheduler by default

The `kiln:actors` entry is now scheduled even when its parent runs on the ordinary single-realm
launcher. A process-shared actor scheduler follows the parent's clock: one deterministic lane for
virtual time, or a bounded wall-clock lane pool. An actor in an existing scheduler uses that
pool. Actor workers reject synchronous `Atomics.wait` with a catchable `TypeError` in both engines;
the native provider also guards the underlying primitive. Ordinary Node-compatible workers keep
dedicated threads and their synchronous wait semantics. This avoids a stackful continuation
project on the path to usable actors.

A focused probe spawned 24 actors, called methods concurrently, parked and resumed each on a
timer, checked the synchronous wait error, and stopped all owners. Native/interpreter parity
matched with zero divergence. A shipped wall-clock binary passed with `KILN_PARALLELISM=1`, and
the ordinary shared-memory worker probe still matched in both engines. Per-realm memory tuning,
the one-realm-per-connection benchmark, and notification-driven native `Atomics.waitAsync` remain
separate work.

### 2026-09-28 -- async atomic waits use the shared waiter list

Native `Atomics.waitAsync` no longer polls the cell. It registers beside synchronous waiters on
the process-wide FIFO list, then waits through a cancellable Tokio host request. Notify wakes the
realm ticket without occupying a scheduler lane, and the only timer is the requested timeout.
The actor extension can use this async operation while keeping its synchronous wait refusal.
The focused worker probe, native GC-stress probe and 1/2/4-realm run passed. The connection
benchmark's latency distribution and full worker scheduling acceptance remain open.

### 2026-09-28 -- HTTP callback affinity on a live TCP socket

The scheduler's HTTP attachment test now uses a loopback TCP connection rather than an
in-memory duplex stream. It sends an HTTP/1.1 request from another OS thread, receives the
complete response, and asserts that every request, close and checkpoint callback runs on the
original scheduler lane. The focused `kiln-runtime` test passed. This closes the live-socket
affinity check; connection response p95 tuning and the dedicated-by-default Node worker
liveness constraint remain open.
