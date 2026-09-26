# 08. Tier 3: worker arenas and allocation

Status: ready-for-human
Blocked by: 06, 07, ADR 0069 accepted

ADR 0069's worker arenas. The collector gains a paused state for the duration of a parallel
section, each worker allocates from private chunks of the kind the nursery uses, and at the end the
realm thread links every chunk into the heap's block table in one step. A worker that exhausts its
arena budget stops, and the call finishes sequentially from that element.

Callbacks may then return new objects (including object literals and spreads of plain data),
arrays and strings. Shapes created on a worker are a question to settle here: either shape
transitions are precomputed on the realm thread from the callback's literal shapes, or workers
create them privately and the join deduplicates them.

A human reviews the design before implementation: it is the largest change to the collector since
the nursery.

## Acceptance

- `rows.parallelMap(r => ({ ...r, total: r.qty * r.price }))` runs in parallel and produces objects
  indistinguishable from `map`'s, including their shapes' property order.
- Every tier 3 probe clean under `mise run native-stress`; no leaks across 10,000 repeated calls.

## Note, 2026-09-26

Joining arenas to the heap is ADR 0070's ownership transfer
(`.scratch/realms-and-concurrency/issues/10-ownership-transfer.md`, step 2). Build the join there.
