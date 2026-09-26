# 04. All realm state reachable from the realm

Status: ready-for-agent
Blocked by: 01 (resolved), 03 for step 5

ADR 0070 needs every piece of realm state to be reachable from the realm, and ADR 0071 decides how:
caches remember layouts, paths and code, never objects; keys and shapes are shared by every realm;
shared state is published atomically and reclaimed at turn boundaries. Issue 01's inventory
(49 realm-state items, 25 caches, 45 process-wide) is the checklist.

Five steps, each its own change, each held to `mise run bench` and the small-service benchmark
within noise of before on one realm, and `scripts/native.sh` at 0 diverged.

1. **Shared keys.** Constant keys (the compiled units' constant tables and the built-ins' keys) are
   interned once per process into a shared, immutable table before the first realm starts;
   `kiln_c99_intern_static` resolves into it. Run-time keys stay in the realm's table. The shared
   table is consulted first.
2. **Shared shapes.** `kiln_c99_root_shape` becomes the one process-wide root, which it already
   is by accident. A transition by a shared key from a shared shape is shared, inserted by
   compare-and-swap, never freed; any other transition is realm-local and belongs to the heap.
   `KILN_KEYS_CHECK=1` keeps its meaning.
3. **Caches restated.** PIC entries drop `holder`, `via` and `holder_value` in favour of prototype
   depth plus shape and slot at that depth, read live from the receiver. The typed-callee cache
   checks the closure's code pointer instead of `_fn`/`_proto`, and reads upvalues from the
   closure. Built-in method caches are valid while the current realm has not modified that
   built-in. `lit_shape`, `kiln_sp_shape_*` and the receiver rows are shape-only already and become
   sound once shapes are shared. `kiln_c99_gc_register_site` loses its reason to exist for entries
   that hold no objects.
4. **Atomic publication and reclamation.** One-word cache state is written and read atomically;
   wider state is an immutable record swapped by compare-and-swap; a replaced record is retired and
   freed once every scheduler thread has passed a turn boundary (quiescent-state reclamation).
   Until the scheduler exists (issue 09), the realm threads of issue 05 are the scheduler threads.
5. **The realm context.** The 49 realm-state items move into a realm-owned context (C runtime
   statics, including the frame registers in `builtins.c`), per-realm per-unit blocks for emitted
   module state (`{sym}_globals`, keys, template sites, graph-launcher namespaces including
   `kiln_namespaces`), and `RealmState` for the Rust items (`node:ffi` `STATE` becomes per realm and
   is cleared on teardown). Generated code loads the context pointer once per function entry
   through the generation-checked cache `kiln_c99_gc_heap` uses. The launcher's shared string
   buffers and last-error global become per realm.

## Acceptance

- Every item in issue 01's inventory classified (a) is reached through the realm context, and
  every (b) either states its answer without objects or lives in the realm's per-unit block.
- A test that grows the shape tree from two heaps in turn (the daemon already hosts several realms
  on one thread) and checks no transition resolves to another realm's key.
- `mise run native-stress` clean after steps 2, 3 and 5.
- Single-realm benchmarks within noise of before.
