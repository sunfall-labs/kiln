# ADR 0049: A compiled process answers SIGUSR1 at the turn boundary

## Status

Accepted. Uses the safepoint ADR 0041 chose, crosses the seams ADR 0028 and ADR 0041 already
built, and reads the connection table ADR 0046 owns.

## Context

`kiln build` produces one native executable. It has no REPL, no inspector, no admin port and no
metrics endpoint, and nothing about it is meant to change: the selling point is what the binary
does not contain. The questions a person can ask such a process today are the ones the operating
system answers — resident size, CPU, open sockets — and none of them distinguishes a server whose
live set is growing from one whose collector is simply working, or a server that is saturated from
one that is parked.

Every number worth having is already in the process:

- `kiln_gc_heap` in the emitted prelude carries `bytes`, `count`, `since_collection`, `survived`,
  `floor`, `epoch`, `collections`, `enabled` and `collecting`. It is a private C struct that
  nothing outside the prelude reads.
- `RealmRuntime::sample_runtime_metrics` produces live tasks, park count, busy duration, elapsed,
  global queue depth and worker count. It is Rust, below the archive boundary, unreachable from
  generated C.
- The two HTTP engines (ADR 0046) each know how many connections they hold and how many requests
  are in flight. `RealmRuntime` published only the boolean `has_open_server_connections`, because
  a boolean was all the drive loop needed.

So the problem is not measurement. It is that the numbers live on two sides of a boundary and
nobody outside the process can ask for either.

## Decision

### `SIGUSR1` writes three lines to stderr and the process keeps serving

```
kiln introspect pid=387175 heap bytes=40098 count=762 since_collection=24531 survived=15567 floor=262144 epoch=143 collections=143 freed_bytes=37522765 enabled=1 collecting=0
kiln introspect pid=387175 runtime live_tasks=0 park_count=22970 busy_us=1656730 elapsed_us=3118322 queue_depth=0 workers=1
kiln introspect pid=387175 connections server=64 client=0 requests_in_flight=0
```

One line per section, `name=value` throughout, so a dump is greppable and diffable and two dumps
subtract. The pid leads every line because `KILN_WORKERS` forks: a signal sent to the group
produces one three-line group per worker, and they have to be separable. Armed by default;
`KILN_INTROSPECT=0` opts out, spelled like `KILN_GC=0`. Arming replaces `SIGUSR1`'s default
disposition, which is to kill the process, so the default is the friendlier one.

### The handler sets a flag; the dump runs at the turn boundary

The handler assigns `1` to a `volatile sig_atomic_t` and returns. Everything else happens at the
top of the native drive loop, beside `collect_if_due` — the safepoint ADR 0041 chose, where no
generated C frame is on the stack and ordinary code may run.

Two independent reasons, either of which would be sufficient:

**The delivering thread is not chosen by us.** A process-directed signal runs on whichever thread
has not blocked it, and a current-thread Tokio runtime still owns a blocking pool. The handler may
therefore execute on a thread that has never touched this heap. Setting a process-global flag is
sound from there; reading a linked list of heap blocks is not. One realm per process — `KILN_WORKERS`
forks rather than threads, because a module's namespaces are process-global while the values in
them belong to the realm that made them (ADR 0038) — makes the *flag* unambiguous. It does not make
the delivering thread predictable, and it would be a mistake to read the second guarantee out of
the first.

**Two of the three sections are not handler-reachable at all.** `runtime` and `connections` cross
into the Rust archive and borrow realm state. Only `heap` could be written from a handler, and a
dump that is two-thirds deferred anyway may as well be three-thirds deferred and internally
consistent.

The formatter is nevertheless written to the async-signal-safe rules: a stack buffer, hand-rolled
base-ten spelling, one `write(2)` per line to fd 2, no `malloc`, no `stdio`, and a bounded retry
instead of an `errno` read. Two of those are load-bearing regardless of signals. `stdio` is the
wrong tool for the reason already recorded beside `kiln_number_to_js_string` — the `printf` family
is locale-sensitive and a diagnostic must not be. One `write` per line is what makes a dump racing
another writer on fd 2 interleave lines rather than characters.

### An armed wall-clock realm never parks longer than 200 ms

A listening server with no timers parks on its completion signal with no deadline. Without a cap
the flag would be observed only when the next connection arrived, which inverts the feature: the
process most worth interrogating is the one that has gone quiet. While the handler is armed, a
**wall-clock** realm's park is capped, so the loop comes round and notices. Measured on an idle
compiled server: 201 ms from `kill -USR1` to the third line.

A virtual-clock realm is never capped. Its park costs no wall time by construction (ADR 0012,
ADR 0028) and its whole point is that a one-hour timer costs no hour; capping it would make
deterministic execution depend on real time, which would surface as flake in the conformance
corpora rather than as a wrong number in a dump. The cap costs an armed idle wall-clock realm five
wakeups a second and nothing at all when it is busy.

### Each side crosses through the seam it already has

The heap layout is prelude-private and the runtime metrics are Rust-private, so at least one side
has to cross. Both do, in opposite directions, using the mechanism that exists for each:

- Rust reaches the prelude through the collector's hooks table. `kiln_gc_hooks` gains
  `introspect`, `KILN_GC_HOOKS_VERSION` goes 1 to 2, and `kiln_realm_gc_attach` refuses a table
  that does not supply it. This is the same channel ADR 0041 built for `collect` and the marker
  hooks, and it is reached through the heap's first word, so it enters exactly one translation
  unit's copy of the prelude.
- The prelude reaches the archive through the private provider ABI. `kiln_realm_runtime_report`
  is a new registry entry, its C declaration generated by `provider_c_declaration!` rather than
  hand-written, and `PROVIDER_ABI_VERSION` moves 28 to 29 as ADR 0028 requires.

The dump text is therefore assembled in exactly one place — the prelude — so there is one format
and one place to change it.

`kiln_realm_runtime_report` takes a slot array with a capacity in and a count out, not a struct.
A struct would have to be declared identically in two languages with nothing keeping it so; the
count lets a prelude built against a shorter report read the prefix it understands and skip the
rest, which is what the ABI version is for at the coarse grain and this is at the fine grain.

### Arming happens in the unit that owns the heap

The prelude is emitted into every translation unit with internal linkage, so a process has N
copies of the flag and N copies of the handler. Exactly one matters: the unit whose candidate heap
won `kiln_realm_gc_attach` is the unit whose static `kiln_c99_gc_hooks` the heap then points at, so
the `introspect` hook the archive calls is that unit's, and the flag it reads is the one that
unit's handler writes. Arming is therefore done at that exact point and nowhere else. Arming in,
say, the heap initialiser would arm every unit's flag and poll only one.

## Consequences

- A production process can be asked what it is doing, for the cost of one signal, and answers
  without stopping. Under four client processes and 64 connections the dump reported `server=64`;
  under `KILN_WORKERS=4` the same load reported 17, 15, 12 and 20 across four pids, which is also
  the first direct read of how `SO_REUSEPORT` is distributing work.
- `park_count`, `busy_us` and `elapsed_us` are interval counters covering the time since the
  previous dump, because `sample_runtime_metrics` is an interval sampler and the dump is its only
  caller in the native path. `bytes`, `count` and the connection counts are instantaneous. Two
  dumps a second apart are a rate; one dump is a level plus a history.
- Every emitted unit now asks for POSIX visibility (`_POSIX_C_SOURCE`) before its first include,
  because `-std=c99` and `-std=c11` put glibc in strict-ANSI mode where `sigaction`, `SA_RESTART`
  and `write` are undeclared. `SIGUSR1` itself is visible without it; the rest is not, and the
  failure reads as `storage size of 'action' isn't known` rather than as a missing header.
- The prelude has its first mutable statics that belong to neither a realm nor a frame. The
  ownership guard grew a `Process` owner to say so: a signal disposition outlives every realm the
  process makes, and a realm must not reset it.
- The turn-boundary restriction is inherited whole from ADR 0041. A CPU-bound synchronous body
  does not dump until it returns, for the same reason it is not collected until it returns. When
  ADR 0021's frame chains land and the collector gains mid-turn safepoints, this gains them too,
  with nothing to undo.

## Alternatives rejected

**Do the work in the handler.** It cannot produce two of the three sections without calling into
Rust and borrowing a `RefCell`, and it would be reading a heap from whichever thread the kernel
picked. What it would buy is a dump from a wedged process that never reaches a turn boundary —
real, but the diagnostic for that is a debugger or a core file, and buying it here would mean
shipping an unsound read for every ordinary dump.

**An admin HTTP port.** It would answer richer questions, and it would put a listening socket, an
authentication decision and a request handler into every binary this project ships. A signal is
already delivered by the operating system, needs no port, cannot be reached from the network, and
composes with `kill -USR1 -- -$pgid` over a forked worker set.

**A Tokio `SignalKind::user_defined1` task instead of a C handler.** Tokio's signal machinery is a
self-pipe the reactor selects on, which would make the idle case immediate with no park cap. It
would also move the trigger to the archive, which is the wrong side: the dump belongs to the
compiled program, the prelude is where it is assembled, and two handlers for one signal is a
last-one-installed race waiting to be discovered. The 200 ms cap buys the same property in four
lines and no new file descriptor.

**Reading the heap struct from Rust instead of adding a hook.** ADR 0041 is explicit that the
prelude owns the heap layout and the archive only calls hooks. Teaching Rust the struct offsets
would make every future field of `kiln_gc_heap` an ABI change in two crates that have no way to
disagree loudly.
