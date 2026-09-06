# Issue 02: O(1) GC Block Sublist Unlinking and Splicing in Native C

Status: planned
Feature: multicore-concurrency
Governing ADR: ADR 0041, ADR 0057

Reality check 2026-09-06 (ADR 0058 review): commit 555de594 changed no file under crates/kiln-backend-c/src/runtime/; there is no splice function in heap.c and no __kiln_transfer_region in either engine.

## Context
Under ADR 0041, every native heap allocation is linked into a doubly-linked list of `kiln_c99_gc_block` structs owned by the realm heap. To enable zero-copy data transfer across realms without serialization or data races, an isolated object subgraph can be transferred in O(1) by unlinking its block sublist from Realm A and splicing it into Realm B.

## Requirements
1. Implement `kiln_c99_gc_splice_sublist(from_heap, to_heap, root_ptr)` in `crates/kiln-backend-c/src/runtime/prelude_gc.c`.
2. Given a verified unaliased root pointer, unlink all blocks belonging to the isolated subgraph from Realm A's intrusive doubly-linked list.
3. Splice the sublist into Realm B's intrusive list in O(1) time.
4. Overwrite the sender's local slot with `undefined` to enforce a destructive read and prevent use-after-transfer.
5. Expose the host operation through `crates/kiln-host-abi` as `__kiln_transfer_region(ptr)`.

## Acceptance Criteria
- Transfer of a 100 MB object graph between realms executes in < 10 nanoseconds (pointer reassignments only).
- Turn-boundary mark-sweep GC runs cleanly in both realms before and after transfer with zero memory corruption under `KILN_GC_STRESS=1`.
- C99 prelude and unit tests pass cleanly under `mbx test -p kiln-backend-c`.
