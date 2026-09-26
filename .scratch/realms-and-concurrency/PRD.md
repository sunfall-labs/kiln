# PRD: Realms and concurrency

Status: needs-triage
Opened: 2026-09-26
Governing ADRs: 0070 (a realm is a schedulable unit, accepted), 0071 (caches remember layouts,
paths and code, never objects), 0040 (the executor is not the realm
state), 0041 (the turn-boundary collector), 0068 (kiln extensions never change Node behaviour),
0069 (parallel built-ins)
Related plans: `.scratch/parallel-builtins/`, `.scratch/deploy-with-kiln/`

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

## Success measures

| measure | 2026-09-26 | target |
| --- | --- | --- |
| memory of one extra realm, beyond the shared region | unknown (issue 01) | tens of KB at creation |
| time to start a realm | unknown | microseconds |
| `worker_threads` | refuses | Node's semantics, in the deploy corpus |
| process-global mutable state in the C runtime and emitted code | ~45 runtime statics, ~57 emitted kinds, `kiln_namespaces` | none outside the realm-validated cache |
| threads the process runs under load, all features on | cores × `KILN_WORKERS` possible | one per core plus the reactor |

## Issues

| # | title | status |
| --- | --- | --- |
| 01 | [Measure what a realm costs and what state is process-global](issues/01-measure-realm-cost.md) | resolved |
| 02 | [`worker_threads` stubs refuse instead of doing nothing](issues/02-worker-threads-stubs-refuse.md) | ready-for-agent |
| 03 | [Implement ADR 0040: the executor split](issues/03-executor-split.md) | ready-for-agent |
| 04 | [All realm state reachable from the realm: shared keys, shared shapes, caches restated, atomic publication, the realm context](issues/04-realm-context.md) | ready-for-agent |
| 05 | [Several pinned realms in one process](issues/05-several-realms-per-process.md) | ready-for-agent |
| 06 | [`node:worker_threads` on realms](issues/06-worker-threads.md) | ready-for-agent |
| 07 | [`KILN_WORKERS` thread mode, and supervision for process mode](issues/07-kiln-workers-thread-mode.md) | ready-for-agent |
| 08 | [The shared immutable region](issues/08-shared-immutable-region.md) | ready-for-agent |
| 09 | [The scheduler: turn migration, pinning, deterministic mode, one pool](issues/09-scheduler.md) | ready-for-agent |
| 10 | [Ownership transfer](issues/10-ownership-transfer.md) | ready-for-agent |
| 11 | [Immutable shared structs: the API](issues/11-immutable-structs-api.md) | needs-triage |
| 12 | [Actors: the API](issues/12-actors-api.md) | needs-triage |
| 13 | [Benchmarks against Node, Bun and Go](issues/13-benchmarks.md) | ready-for-agent |

ADR 0070 was accepted on 2026-09-26; nothing here waits on a decision except the two API ADRs
(11, 12).
