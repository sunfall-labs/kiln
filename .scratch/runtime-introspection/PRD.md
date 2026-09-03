# PRD: Runtime introspection for a natively compiled server

Status: done
Owner: Claude (`worktree-agent-a998e1ca35c141473`)
Opened: 2026-09-03

## Problem

A `kiln build`-produced server is a single native binary with no REPL, no inspector and no admin
port. When one of them is holding memory, or serving slowly, or apparently doing nothing, the only
questions we can currently ask it are the ones the OS answers: RSS, CPU, open sockets. None of
those say whether the collector is keeping up, how many turns the realm has taken, or how many
HTTP connections the engine still has open.

Everything we want is already in the process. It is just not reachable from anywhere a person can
poke:

- `kiln_gc_heap` (prelude, ADR 0041) has `bytes`, `count`, `since_collection`, `survived`, `floor`,
  `epoch`, `collections`, `enabled`, `collecting`. Nothing outside the prelude reads it.
- `RealmRuntime::sample_runtime_metrics` produces `live_tasks`, `park_count`, `busy_duration`,
  `elapsed`, `global_queue_depth`, `workers`. It is Rust-private; generated C cannot call it.
- `HttpServerEngine::open_connections` / `HttpClientEngine::open_connections` and
  `RealmRuntime::requests_in_flight` know the connection picture (ADR 0046). `RealmRuntime`
  exposed only the boolean `has_open_server_connections`.

## What we build

`SIGUSR1` makes the process write three lines to stderr and keep serving:

```
kiln introspect pid=<pid> heap bytes=… count=… since_collection=… survived=… floor=… epoch=… collections=… enabled=… collecting=…
kiln introspect pid=<pid> runtime live_tasks=… park_count=… busy_us=… elapsed_us=… queue_depth=… workers=…
kiln introspect pid=<pid> connections server=… client=… requests_in_flight=…
```

`pid=` is first because `KILN_WORKERS>1` forks (see below): a signal sent to the process group
produces one three-line group per worker and they have to be separable.

## Design

### The handler sets a flag; the dump runs at the turn boundary

Straight from ADR 0041. The `SIGUSR1` handler does the one thing a handler may do — assign to a
`volatile sig_atomic_t` — and the dump runs at the top of the native drive loop, the same
safepoint the collector sweeps at, where no generated C frame is on the stack and ordinary code
may run.

This is not only stylistic. Two of the three sections cannot be produced from a handler at all:
`runtime` and `connections` cross into the Rust archive and borrow realm state. Only the `heap`
section could be written from the handler, and a dump that is two-thirds deferred anyway may as
well be three-thirds deferred and internally consistent.

The formatter is nevertheless written to the async-signal-safe rules — a stack buffer, hand-rolled
integer spelling, one `write(2)` per line, no `malloc`, no `stdio` — because the `printf` family
would be the wrong tool here anyway for the reason recorded beside `kiln_number_to_js_string`
(locale), and because one `write` per line means a dump racing another writer on fd 2 interleaves
lines rather than characters.

### One realm per process makes the flag unambiguous, but not the thread

`KILN_WORKERS` forks (NOTES.md, 2026-09-03): extra realms are processes, not threads, because a
module's namespaces are process-global while the values in them belong to the realm that made them
(ADR 0038). So the realm whose heap we read is the only realm in the address space.

The *thread* is a different question. The kernel delivers a process-directed signal to any thread
that has not blocked it, and a current-thread Tokio runtime still owns a blocking pool. The
handler may therefore run on a thread that has never touched the heap. Setting a process-global
flag is sound from there; reading the heap would not be. This is an independent reason the handler
must not do the work, and it is not fixed by there being one realm.

### An idle server dumps within 200 ms, not at its next request

A listening server with no timers parks in `wait_for_turn(signal, None)` — indefinitely. The flag
would then be observed only when the next request arrived, which is exactly backwards: the process
you most want to interrogate is the one that has stopped answering.

While the handler is armed, a **wall-clock** realm's park is capped at 200 ms, so the drive returns
to the top of its loop and notices the flag. A virtual-clock realm is never capped: its park costs
no wall time by construction (ADR 0012 / ADR 0028) and capping it would make deterministic
execution depend on real time. The cost when armed and idle is five wakeups a second.

### Crossing the two seams

The heap layout is prelude-private and the runtime metrics are Rust-private, so at least one of the
two has to cross. Both do, each in its own direction, through the mechanism that already exists for
it:

- Rust reaches the prelude through the collector's hooks table (ADR 0041): `kiln_gc_hooks` gains
  `introspect`, and `KILN_GC_HOOKS_VERSION` goes 1 → 2.
- The prelude reaches the archive through the private provider ABI: a new export
  `kiln_realm_runtime_report`, declared by an arm of `provider_c_declaration!` rather than a
  hand-written `extern`, with `PROVIDER_ABI_VERSION` 28 → 29.

The dump text is assembled in one place, the prelude, so there is one format and one place to
change it.

### Arming

Armed by default in every compiled binary; `KILN_INTROSPECT=0` opts out, spelled like `KILN_GC=0`.
Arming replaces `SIGUSR1`'s default disposition, which is to kill the process, so the default is
strictly the friendlier one. It happens once, in the translation unit that wins the race to create
the realm heap — the same unit whose static `kiln_c99_gc_hooks` the heap then points at, so the
flag the handler sets and the flag the hook reads are the same object even though the prelude is
emitted per unit with internal linkage.

## What we could not reach

Nothing on the list. Connection counts were reachable: `RealmState` owns both engines and both
already counted their live connections; only the accessors were missing from `RealmRuntime`.

Two honest limits worth recording:

- `park_count`, `busy_us` and `elapsed_us` are **interval** counters. `sample_runtime_metrics`
  reports them since the previous call, and the dump is the only caller in the native path, so they
  read "since the previous dump", or since runtime construction for the first one. `bytes`, `count`
  and the connection counts are instantaneous.
- A CPU-bound synchronous body still does not dump until it returns, for exactly the reason ADR
  0041 gives for not collecting there: there is no safepoint inside generated code yet.

## Issues

| # | Title | Status |
| --- | --- | --- |
| 01 | Reach the runtime and connection numbers from generated C | done |
| 02 | A collector hook for the turn-boundary dump | done |
| 03 | The signal handler, the flag and the formatter | done |
| 04 | An idle wall-clock server must dump within a bounded time | done |
| 05 | Verify and demonstrate on a loaded server | done |

## Demonstration

`bench/server/serve.mjs` compiled with `kiln build --node --profile native-release`, put under
`bash bench/server/drive-multi.sh 4 16 5000 18088 hello`, signalled twice during the load and once
a second after it stopped. Verbatim stderr:

```
kiln introspect pid=387175 heap bytes=40098 count=762 since_collection=24531 survived=15567 floor=262144 epoch=143 collections=143 freed_bytes=37522765 enabled=1 collecting=0
kiln introspect pid=387175 runtime live_tasks=0 park_count=22970 busy_us=1656730 elapsed_us=3118322 queue_depth=0 workers=1
kiln introspect pid=387175 connections server=64 client=0 requests_in_flight=0
kiln introspect pid=387175 heap bytes=46128 count=882 since_collection=30561 survived=15567 floor=262144 epoch=335 collections=335 freed_bytes=87923917 enabled=1 collecting=0
kiln introspect pid=387175 runtime live_tasks=0 park_count=33189 busy_us=1831902 elapsed_us=2013290 queue_depth=0 workers=1
kiln introspect pid=387175 connections server=64 client=0 requests_in_flight=0
kiln introspect pid=387175 heap bytes=265118 count=5354 since_collection=249551 survived=15567 floor=262144 epoch=441 collections=441 freed_bytes=115749553 enabled=1 collecting=0
kiln introspect pid=387175 runtime live_tasks=0 park_count=28160 busy_us=869692 elapsed_us=2213368 queue_depth=0 workers=1
kiln introspect pid=387175 connections server=0 client=0 requests_in_flight=0
```

The run itself: `{"processes": 4, "connections": 64, "rps": 57710, "errors": 0, "wrong": 0,
"p50_ms": 0.848, "p99_max_ms": 3.516}` — no errors, so the signals did not disturb it.

What the three dumps say, which is the point of the exercise:

- `connections server=64` twice, then `0`. That is exactly 4 processes × 16 connections, and the
  third dump is after the client let go.
- `survived=15567` in all three. The live set is flat across 441 collections and ~115 MB freed:
  the collector is keeping up and the heap is a function of live objects, not requests served,
  which is what ADR 0041 promised and what nothing outside a bench could previously check.
- `busy_us / elapsed_us` is 53% in the first interval, 91% in the second, 39% in the third. The
  interval semantics show through here: the first covers the process's whole life to that point.
- `since_collection` under `floor` at the moment of the dump is the collector's own threshold
  visible from outside.

### Idle, and the 200 ms bound

Same binary, no traffic at all, timed from the signal:

```
elapsed_ms=201
kiln introspect pid=383895 heap bytes=1449 count=48 since_collection=1481 survived=0 floor=262144 epoch=0 collections=0 freed_bytes=32 enabled=1 collecting=0
kiln introspect pid=383895 runtime live_tasks=0 park_count=197 busy_us=710 elapsed_us=13273226 queue_depth=0 workers=1
kiln introspect pid=383895 connections server=0 client=0 requests_in_flight=0
```

`collections=0` and `survived=0`: the server had never allocated enough to collect once. A dump is
useful before anything has happened, which is not true of RSS.

### Four workers

`KILN_WORKERS=4`, same load, one signal to the process group:

```
kiln introspect pid=388521 connections server=17 client=0 requests_in_flight=0
kiln introspect pid=388523 connections server=15 client=0 requests_in_flight=0
kiln introspect pid=388524 connections server=12 client=0 requests_in_flight=0
kiln introspect pid=388525 connections server=20 client=0 requests_in_flight=0
```

(heap and runtime lines elided; the full four-by-three dump is in issue 05.) 17 + 15 + 12 + 20 = 64,
which is both a check on the feature and the first direct read of how `SO_REUSEPORT` is splitting
the load.
