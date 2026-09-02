# 01 — A heap-owned, weak intern table for property keys

Status: done

## What

Add to `kiln_gc_heap` (prelude, `crates/kiln-backend-c/src/lib.rs`) an open-addressed table
mapping key bytes to one canonical pointer, and three entry points:

- `kiln_c99_intern(bytes, len)` — canonical pointer, inserting. Adopts a `KILN_GC_STRING` block
  whose size is exactly `len` as the canonical bytes; copies anything else into a fresh block.
- `kiln_c99_intern_static(bytes, len)` — canonical pointer, inserting, for immortal bytes. Uses the
  bytes themselves when new; pins the entry either way.
- `kiln_c99_interned(bytes, len)` — canonical pointer or `NULL`, never inserting.

The empty key is a pinned static `""` so pointer identity holds for it too. No two canonical
pointers may share an address (the exact-size rule on adoption is what guarantees it).

## Collector

- Mark phase: pinned entries whose bytes are blocks are marked (`kiln_c99_gc_collect`, after the
  runtime roots and before drain; the same at teardown).
- After drain and before sweep: drop every unpinned entry whose block is unmarked (rebuild).
- `kiln_c99_gc_release` on a `KILN_GC_STRING` block forgets the entry that names it.
- `kiln_c99_gc_teardown_end` frees the table storage with the heap.
- Skip the purge when `mark_failed` (nothing is freed then either).

## Why in the heap struct

The heap is the one C struct every translation unit shares and the runtime archive keeps
(ADR 0041); the table has to be shared the same way or two units would have two canonical
pointers for one name. The Rust side reads only the hooks word, so the layout can grow.

## Done when

- `cargo test -p kiln-backend-c` passes, including the mutable-static owner test (no new
  `kiln_c99_*` mutable statics: the table lives in the heap).
- A prelude unit test interns the same bytes from three sources and gets one pointer.

## Comments
