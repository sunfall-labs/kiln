# PRD: Realms and concurrency

Status: resolved for the shipping scope (remaining broad bootstrap sharing deferred; historical TypeScript startup measurement unavailable)
Opened: 2026-09-26
Governing ADRs: 0070 (a realm is a schedulable unit, accepted), 0071 (caches remember layouts,
paths and code, never objects), 0040 (the executor is not the realm
state), 0041 (the turn-boundary collector), 0068 (kiln extensions never change Node behaviour),
0069 (parallel built-ins)
Related plans: `docs/issues/parallel-builtins/`, `docs/issues/deploy-with-kiln/`

## Problem

kiln has five unrelated concurrency mechanisms (ADR 0070, Context): forked processes for
`KILN_WORKERS`, an unused `WorkerPool`, a separate Rayon pool for `parallelMap`, proxy actors with
identity `isolate`/`transfer`, and a `node:worker_threads` whose `Worker` throws and whose
`BroadcastChannel` and fallback `MessageChannel` silently do nothing.

## Goal

ADR 0070: realms scheduled across one set of scheduler threads at turn granularity, with three
primitives (messages, the shared immutable region, ownership transfer), and every concurrency
feature built from them.

What that must deliver, and against what:

| capability | Node / Bun | target |
| --- | --- | --- |
| a worker | a full isolate: tens of MB, milliseconds to start | a realm: tens of KB beyond the shared region, microseconds to start |
| sending data | copy | copy, or transfer an object graph without copying |
| sharing data | bytes only (`SharedArrayBuffer`) | bytes, plus immutable object graphs with zero copy |
| data-parallel map | a hand-built worker pool that copies | `parallelMap` over the frozen heap, no copies |
| granularity | a few workers | a realm per actor, connection or tenant, as Erlang processes and goroutines allow |

The targets in the second and last rows are the ones to measure first; issue 01 sets the baseline.

## Non-negotiable

- Inside one realm nothing changes: single-realm programs, Test262 and the Node corpora are
  unaffected by construction, and `scripts/native.sh` stays at 0 diverged throughout.
- `worker_threads` and everything Node defines keep Node's semantics exactly.
- Multi-realm programs stay reproducible in the deterministic mode.

## Out of scope

- CI changes.
- Shared mutable objects across realms (ADR 0070 rejects them).
- The actor and immutable-struct *APIs*: each gets its own ADR (issues 11, 12). This plan builds
  what they stand on.

## Order

1. **Measure** (01): what a realm costs today and what state is process-global.
2. **Stop the silent answers now** (02): the `worker_threads` stubs refuse instead of doing nothing.
3. **ADR 0040** (03): the executor split. A throughput win on its own, and the first step here.
4. **Realm context** (04): all realm state reachable from the realm. Then several pinned realms per
   process (05).
5. **Node first** (06, 07): `worker_threads` with transfer lists, and the thread mode of
   `KILN_WORKERS` with supervision for process mode.
6. **Make realms cheap** (08): the shared immutable region, starting with built-ins and bootstrap.
7. **The scheduler** (09): turn migration, pinning, the deterministic mode, and one pool for turns
   and parallel chunks.
8. **Transfer** (10), then the **APIs** (11, 12), then **benchmarks** (13) at every step.

### Delivery priority after the actor API landed (2026-09-28)

The actor API and `kiln:shared` now exist. The bounded actor scheduler, startup path and
module-addressed call reliability have landed. Public actor capacity now exposes the memory work:
the held 100-to-1,000 slope fell from about 816 to about 530 KiB per actor after direct
process-environment construction, versus about 97 KiB for a minimal core realm. Focus issue 08
on the retained worker/module bootstrap graph. Ordinary actor messages
use structured clone; `share()` supplies explicit immutable snapshots; `ArrayBuffer` transfer
supplies the constant-size byte handoff. This is the usable baseline to benchmark, document and
run through the final acceptance gate. The broader bootstrap blueprint is a separate density
and startup improvement; do not hold ordinary actor use behind that representation change.

`prepareTransfer(value)` is an optional, explicit snapshot into a private transfer arena for
supported plain graphs. Its preparation copy is charged to the sender. Broaden its accepted graph
only with interpreter/native parity, GC stress and measured advantage over structured clone at
relevant sizes. Direct `isolate(factory)` graph moves still require the complete alias, storage and
turn-boundary proof in issue 10; they are not a prerequisite for the ordinary actor API.

Keep Node-compatible workers on dedicated threads while synchronous `Atomics.wait` retains its
stack, as ADR 0070's amendment records. Actor realms use the bounded scheduler because their API
refuses synchronous waits. Issue 09's 100,000-realm capacity and connection-latency acceptance
is recorded. Issue 13's capacity and three-round non-server and HTTP comparisons are measured.
The socket-capable program gate and collector stress passed on `e4bda4df`
(see [verification](program-verification.md)). Issue 08's broad density work remains.
The architecture review in `bootstrap-image-design.md` makes the process-owned bootstrap
blueprint and realm-local identity/value overlays the primary density path. A copied bootstrap
heap is deferred unless a measured startup benefit justifies its separate relocation protocol.

### Shipping scope after bootstrap cost review

The user approved deferring broad bootstrap sharing and shipping the usable actor/parallel
implementation after final transfer verification. Issue 08 retains its measurements and unmet
density/startup targets as deferred work. Its blueprint draft is not active in the runtime.

## Success measures

| measure | 2026-09-26 | target |
| --- | --- | --- |
| memory of one extra realm, beyond the shared region | unknown (issue 01) | tens of KB at creation |
| time to start a realm | unknown | microseconds |
| `worker_threads` | pinned pino and piscina fixtures match Node in both engines; native notify-only `waitAsync` now wakes through the shared waiter list; weak cleanup remains | Node's semantics, in the deploy corpus |
| process-global mutable state in the C runtime and emitted code | ~45 runtime statics, ~57 emitted kinds, `kiln_namespaces` | none outside the realm-validated cache |
| threads for async-only scheduled realms and parallel chunks | cores × `KILN_WORKERS` possible | one per core plus the reactor (Node workers with synchronous waits use dedicated threads) |

## Issues

| # | title | status |
| --- | --- | --- |
| 01 | [Measure what a realm costs and what state is process-global](issues/01-measure-realm-cost.md) | resolved |
| 02 | [`worker_threads` stubs refuse instead of doing nothing](issues/02-worker-threads-stubs-refuse.md) | resolved |
| 03 | [Implement ADR 0040: the executor split](issues/03-executor-split.md) | resolved |
| 04 | [All realm state reachable from the realm: shared keys, shared shapes, caches restated, atomic publication, the realm context](issues/04-realm-context.md) | resolved |
| 05 | [Several pinned realms in one process](issues/05-several-realms-per-process.md) | resolved |
| 06 | [`node:worker_threads` on realms](issues/06-worker-threads.md) | resolved (pinned deploy acceptance; semantic gaps recorded) |
| 07 | [`KILN_WORKERS` thread mode, and supervision for process mode](issues/07-kiln-workers-thread-mode.md) | resolved |
| 08 | [The shared immutable region](issues/08-shared-immutable-region.md) | deferred (remaining broad bootstrap sharing; existing improvements landed; ~97 KiB/core realm and ~530 KiB/held public actor) |
| 09 | [The scheduler: turn migration, pinning, deterministic mode, one pool](issues/09-scheduler.md) | resolved (bounded async turns and parallel chunks; concurrent actor stop fixed; program gate and stress passed) |
| 10 | [Ownership transfer](issues/10-ownership-transfer.md) | resolved (candidate-owner proof, strict oracle agreement, scaling, gate with isolated timing recheck, collector stress and repeated realms verified) |
| 11 | [Immutable shared structs: the API](issues/11-immutable-structs-api.md) | resolved (program gate and stress passed) |
| 12 | [Actors: the API](issues/12-actors-api.md) | resolved (object-graph handoff remains 10) |
| 13 | [Benchmarks against Node, Bun and Go](issues/13-benchmarks.md) | resolved (three-round non-server and HTTP comparisons refreshed; capacity targets measured) |
| 14 | [In-realm messaging at Node's speed](issues/14-in-realm-messaging-speed.md) | resolved |
| 15 | [Many concurrent tasks in one realm](issues/15-many-tasks-scaling.md) | resolved |
| 16 | [`{ code: i * 3 }` in a loop fails to compile natively](issues/16-object-literal-in-loop-compile-bug.md) | resolved |
| 17 | [Symbol-keyed properties on the fast path](issues/17-symbol-keyed-property-fast-path.md) | resolved |
| 18 | [The native backend reports unhandled rejections](issues/18-native-unhandled-rejections.md) | resolved |
| 19 | [Node-parity gaps found by the messaging work](issues/19-parity-gaps-found-by-messaging.md) | resolved |
| 20 | [Writing a built-in makes typed bodies refuse](issues/20-writing-a-builtin-makes-typed-bodies-refuse.md) | resolved |
| 21 | [Cross-realm messaging at node's speed](issues/21-cross-realm-messaging-speed.md) | resolved (four paired rows meet their ceilings; program gate passed) |
| 22 | [Interpreter: "local slot 0 does not hold a captured cell"](issues/22-interpreter-captured-cell-slot.md) | resolved |
| 23 | [`process.on(signal)` in realms, for graceful shutdown](issues/23-process-signal-events.md) | resolved |
| 24 | [A segfault with several realms in one process](issues/24-segfault-with-several-realms.md) | resolved |
| 25 | [`BroadcastChannel` is scoped to one program, not to the process](issues/25-broadcast-channel-scope-across-realms.md) | resolved |
| 26 | [A program run inside the daemon must not signal the daemon](issues/26-daemon-hosted-program-signals-itself.md) | resolved |

ADR 0070 was accepted on 2026-09-26. The API decisions for issues 11 and 12 are recorded in
ADRs 0073 and 0072, respectively; neither remains a decision gate.

## Performance checks owed

The 2026-09-29 quiet class-polymorphism audit found a 3.44 ms current workload
against 1.91 ms immediately before step 3. Restoring guarded captured-array
specialization, separately from unconditional dead-zone elimination, brings it to
1.78 ms in three alternating rounds with matching checksums: about 48% faster than
the unfixed current workload and 7% faster than the historical endpoint. See the
[historical performance audit](performance-audit.md) for proof limits and records.
The executor split's quiet three-round before/after check is also complete:
hello and mixed-service throughput differ by less than 0.5%, with no response
errors or CPU/request regression.
The shared-keys/shapes audit is complete for the selected native hot paths: its historical
JSON slowdown is absent in current production (62 ms versus 75 ms before sharing), and
current hello/mixed HTTP throughput differs by less than 0.3%, with lower CPU/request.
The audit preserves the isolated historical results and the unsupported historical
object-spread build rather than hiding them.

Changes to hot paths landed while the machine was loaded by parallel builds, so their benchmarks
were inconclusive. They are re-measured together on a quiet machine before this plan closes:

- step 1b's key registration before `main`: TypeScript bundle startup remains unmeasured because
  the historical compiler failed the 6 GiB cap at both endpoints and the 10 GiB post-change retry.
  The paired Babel audit is recorded, with overlapping before/after timing ranges.

The issue-14 structured-clone startup audit is complete for external elapsed time and binary
size (hardware counters were not collected). Its historical interpreter loop slowdown is absent
in current production, which is about 34% faster than the pre-change endpoint. Current Node
native hello starts about 0.54 ms slower and is larger than that endpoint; this includes all
intervening changes and feeds issue 08's remaining bootstrap work. Exact pairs, inputs, limitations
and raw cells are in [the historical audit](performance-audit.md).
