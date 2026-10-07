# Parallel nursery chunk generation initialization

`kiln_c99_par_arena_refill` allocates worker chunks through `kiln_c99_arena_chunk` and the shared
`kiln_c99_chunk_lay_out`, bypassing `kiln_c99_nursery_use`, which normally resets `tenured` to zero.
The layout routine initialized the rest of the chunk header but left `tenured` as whatever byte was
already in the aligned region. A minor collection therefore classified newly adopted worker
chunks as old, skipped marking their children, and settled them as dead.

The regression roots a parent array through `realm.isolated_graphs`, gives it a child array from an
`0xA5`-poisoned parallel chunk, then requests a mid-turn collection. It checks that the collection
was actually minor through `minor_audit_collections` and that the child remains allocated with its
payload intact. With the exact pre-fix `kiln_c99_chunk_lay_out` restored from the source parent, the
test fails because the child lookup returns null. With the fix, it passes. The production fix zeros
the whole shared header before laying out pointers and counters, so all new chunk metadata starts
from a known state.

The earlier GDB census on the unchanged failing native binary independently found that the rooted
answers array was marked in epoch 10 while 615 mapped child arrays were allocated but unmarked.
The first affected chunk had `candidate_state=0`, `tenured=12` (a later run read `10`), and stale
mark epoch 0. Source layout and generated disassembly place `tenured` at chunk byte 105 and show
`gc_mark_found` testing `minor_collection` at heap offset 0xfc and consulting that chunk byte. The
mid-turn `new_array_with_capacity -> par_flatten` stack identified the collection window. Raw GDB
inventory and watch logs remain in `.scratch/parallel-alloc-diagnostic/` in the working tree.

The candidate CLI was built from source commit `1caec69656a10dd4f2c71c0a3cf10205735658fd` with
`MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- mbx build --release -p kiln-cli`.
It is `target/release/kiln`, SHA-256
`d2ac001eaf67ee68c09e193244ac0afad3de51ea5545f44fc060e425aec37533`, mtime
`2026-10-04 16:35:10 -0600`. The native provider manifest SHA-256 is
`b03c48a204de3f911b424ce62b7d96a88ef595de0f99217cba8c34caf6c5dd6e`; its archive SHA-256 is
`61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.

Using that CLI and provider, `scripts/native.sh` matched all three original probes:
`parallel-alloc-budget.js`, `parallel-alloc-objects.js`, and `parallel-alloc-flatmap.js` (3 matched,
0 refused, 0 diverged). The dedicated `parallel-alloc-stress.js` matched under
`KILN_NATIVE_GC_STRESS=1` (1 matched, 0 refused, 0 diverged, 0 skipped). That fixture now retains
the nested arrays returned by `parallelFlatMap` across a sequential `flatMap` replay and validates
the child values after the allocations that exposed the bug.

Raw red/green backend logs and both native summaries are checked in beside this note.
