# PRD: Parallel built-ins that are extremely effective, and never change Node behaviour

Status: needs-triage
Opened: 2026-09-26
Governing ADRs: 0068 (kiln extensions never change Node behaviour, accepted), 0069 (parallel
collection built-ins run over a frozen heap, proposed), 0058 (application 2), 0041 (the collector)

## Relation to ADR 0070

The parallel built-ins run on ADR 0070's architecture (`.scratch/realms-and-concurrency/`): chunks
run on its scheduler, the no-write read path (06 here) is built once as its shared immutable
region's read path (realms issue 08), and worker arenas (08 here) join the heap through its
ownership transfer (realms issue 10). Issues 01 to 05 here do not depend on any of that.

## Problem

`Array.prototype.parallelMap` and the typed-array `parallelMap`s exist in both engines. The native
backend runs a callback in parallel only when `crates/kiln-backend-c/src/parallel.rs` proves it
takes numbers, returns a number, and touches nothing else: no captured variable, no property read,
no allocation, no call except a listed `Math` function. Almost nothing a developer writes
qualifies. The rest runs sequentially and correctly, but with no benefit and no explanation.

Separately, nothing records that `parallelMap` is kiln's own member, so the surface inventory
counts it beside real divergences, and there is no way to run with exactly Node's surface.

## Goal

Any `map`-like callback a developer would reasonably write runs across every core with results
identical to the sequential method; when one cannot, `kiln build --explain-parallel` says exactly
why; and a program that uses only Node's surface can never tell the extensions exist.

## Non-negotiable

- **No Node behaviour changes** (ADR 0068). Results, exceptions and side effects are identical to
  the sequential method. A callback with side effects runs sequentially; there is no relaxed mode.
- **Deterministic.** No answer depends on core count or scheduling, in either engine.
- **Never slower than `map`** for small inputs: the cost model keeps those sequential.

## Out of scope

- CI changes.
- In-process realms on threads, actors and `worker_threads`. The no-write read path and worker
  arenas built here are what those would later need (ADR 0057), but they are not planned here.
- Process workers (`KILN_WORKERS`) beyond sizing the pool against them (issue 04).

## Tiers

| tier | what qualifies after it | issues |
| --- | --- | --- |
| today | numbers in, number out, no captures, `Math` only | |
| 1 | + reading captured values that are never reassigned | 05 |
| 2 | + reading arrays, typed arrays, object properties and strings | 06, 07 |
| 3 | + returning new objects, arrays and strings | 08 |
| 4 | + calling pure functions and pure built-ins | 09 |
| family | `parallelFilter`, `parallelFlatMap`, `parallelReduce`, `parallelSort` | 10 |

## Success measures

- Share of `parallelMap` call sites in `bench/` and the deploy corpus that run in parallel, per
  tier, from `--explain-parallel`.
- Speedup over `map` on realistic workloads at 1, 4 and all cores, measured warm (issue 11).
- Zero Node divergences caused by extensions across Test262, `conformance/thirdparty` and the
  deploy corpus (issue 01).
- `mise run native-stress` clean on every parallel probe after tiers 2 and 3.

## Order

1. **Guarantees first** (01, 02): the extension mark and switch, and the differential harness
   that holds every parallel result to the sequential one. Nothing ships in a tier without them.
2. **Visibility and sizing** (03, 04): `--explain-parallel`, the cost model and the thread budget.
   Cheap, and they make every later tier measurable.
3. **Tiers 1 to 4** (05 to 09), in order.
4. **The family** (10), once tier 3 lands, since `filter` and `flatMap` allocate.
5. **Proof** (11, 12): benchmarks and user documentation, updated at each tier.

## Issues

| # | title | status |
| --- | --- | --- |
| 01 | [Mark kiln extensions and add the switch that removes them](issues/01-extension-mark-and-switch.md) | ready-for-agent |
| 02 | [The parallel/sequential differential harness](issues/02-differential-harness.md) | ready-for-agent |
| 03 | [`kiln build --explain-parallel`](issues/03-explain-parallel.md) | ready-for-agent |
| 04 | [Cost model and thread budget](issues/04-cost-model-and-thread-budget.md) | ready-for-agent |
| 05 | [Tier 1: read captured values](issues/05-tier-1-captured-values.md) | ready-for-agent |
| 06 | [The no-write read path](issues/06-no-write-read-path.md) | ready-for-human |
| 07 | [Tier 2: read the heap](issues/07-tier-2-read-the-heap.md) | ready-for-agent |
| 08 | [Tier 3: worker arenas and allocation](issues/08-tier-3-worker-arenas.md) | ready-for-human |
| 09 | [Tier 4: pure calls](issues/09-tier-4-pure-calls.md) | ready-for-agent |
| 10 | [The parallel family](issues/10-parallel-family.md) | ready-for-agent |
| 11 | [Benchmarks on realistic workloads](issues/11-benchmarks.md) | ready-for-agent |
| 12 | [User documentation](issues/12-user-documentation.md) | ready-for-agent |

06 and 08 are marked for a human because they change the collector's contract and need ADR 0069
accepted, with its mechanism reviewed, before they start.
