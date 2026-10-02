# PRD: Parallel built-ins that are extremely effective, and never change Node behaviour

Status: resolved for the planned shipping scope, including the issue 29 follow-up
Opened: 2026-09-26
Governing ADRs: 0068 (kiln extensions never change Node behaviour, accepted), 0069 (parallel
collection built-ins run over a frozen heap, accepted), 0058 (application 2), 0041 (the collector)

## Relation to ADR 0070

The parallel built-ins run on ADR 0070's architecture (`docs/issues/realms-and-concurrency/`): chunks
run on its scheduler, the no-write read path (06 here) is built once as its shared immutable
region's read path (realms issue 08), and worker arenas (08 here) join the heap through its
ownership transfer (realms issue 10). Issues 01 to 05 here do not depend on any of that.

## Problem

`Array.prototype.parallelMap` and the typed-array `parallelMap`s exist in both engines. The native
backend runs a callback in parallel only when `crates/native/kiln-backend-c/src/parallel.rs` proves it
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
| 01 | [Mark kiln extensions and add the switch that removes them](issues/01-extension-mark-and-switch.md) | resolved |
| 02 | [The parallel/sequential differential harness](issues/02-differential-harness.md) | resolved |
| 03 | [`kiln build --explain-parallel`](issues/03-explain-parallel.md) | resolved |
| 04 | [Cost model and thread budget](issues/04-cost-model-and-thread-budget.md) | resolved |
| 05 | [Tier 1: read captured values](issues/05-tier-1-captured-values.md) | resolved |
| 06 | [The no-write read path](issues/06-no-write-read-path.md) | resolved |
| 07 | [Tier 2: read the heap](issues/07-tier-2-read-the-heap.md) | resolved |
| 08 | [Tier 3: worker arenas and allocation](issues/08-tier-3-worker-arenas.md) | resolved |
| 09 | [Tier 4: pure calls](issues/09-tier-4-pure-calls.md) | resolved |
| 10 | [The parallel family](issues/10-parallel-family.md) | resolved |
| 11 | [Benchmarks on realistic workloads](issues/11-benchmarks.md) | resolved |
| 12 | [User documentation](issues/12-user-documentation.md) | resolved |
| 13 | [Parallel code in every module and every build profile](issues/13-parallel-code-in-every-module.md) | resolved |
| 14 | [Parallel workers compiled optimised in generic-only modules](issues/14-parallel-workers-optimised-in-generic-modules.md) | resolved |
| 15 | [A parallel method's fallback is never slower than its sequential method](issues/15-fallback-never-slower-than-sequential.md) | resolved |
| 16 | [Sequential typed-array methods at node's speed](issues/16-sequential-typed-array-methods-at-node-speed.md) | resolved |
| 17 | [Bootstrap built-ins compiled optimised](issues/17-bootstrap-built-ins-compiled-optimised.md) | resolved |
| 18 | [Release miscompile: a loop's assignment is lost after a string append](issues/18-release-miscompile-lost-assignment.md) | resolved |
| 19 | [The release source DCE drops coercions and throwing calls it believes are pure](issues/19-release-source-dce-drops-observable-coercions.md) | resolved |
| 20 | [An integer remainder loses negative zero through its integer twin](issues/20-integer-remainder-loses-negative-zero.md) | resolved |
| 21 | [A typed method call refuses a receiver its typed clone cannot take](issues/21-typed-method-call-refuses-a-receiver-its-typed-clone-cannot-take.md) | resolved |
| 22 | [A one-thread pool takes the sequential path](issues/22-one-thread-pool-takes-the-sequential-path.md) | resolved |
| 23 | [An array's string conversion does not call its elements' `toString`](issues/23-array-element-tostring-skips-user-code.md) | resolved |
| 24 | [The native backend's computed key does not call `ToPropertyKey`'s user code](issues/24-native-computed-key-skips-to-property-key.md) | resolved |
| 25 | [The native backend accepts a class heritage that is not a constructor](issues/25-native-class-heritage-accepts-a-non-constructor.md) | resolved |
| 26 | [The interpreter's `flatMap` keeps an answer's holes](issues/26-interpreter-flatmap-keeps-holes.md) | resolved |
| 27 | [A release arithmetic branch reads an unwritten box](issues/27-release-remainder-condition-takes-one-arm.md) | resolved |
| 28 | [Class and literal parity gaps found by issues 23-25](issues/28-class-and-literal-parity-gaps.md) | resolved |
| 29 | [Function, instanceof and bound-function parity gaps](issues/29-function-and-instanceof-parity-gaps.md) | resolved |
| 30 | [Native built-in method calls ignore replaced prototype methods, a UTF-16 `split` limit and UTF-16 `parseInt`](issues/30-native-ignores-replaced-prototype-methods.md) | resolved |

ADR 0069 was accepted on 2026-09-26. Issues 06 and 08 use ADR 0070's shared region and transfer
(realms issues 08 and 10).
