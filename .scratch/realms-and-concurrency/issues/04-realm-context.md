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
1b. **Register constant keys before the first realm.** Step 1 shares a constant key only while a
   single heap is live, because nothing registers a program's constants before `kiln_realm_create`.
   The launcher registers every unit's constant keys (and the built-ins') into the shared table
   before the first realm starts, so every realm's constants are shared, as ADR 0071 says.
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

## Comments

### 2026-09-26 -- steps 1 and 2 done (branch `alee/shared-keys-shapes`, commit 8b0dc1e2)

**Where the shared state lives.** One process-wide block reserved by the provider archive and
reached through a new ABI entry, `kiln_realm_process_shared(size)` (provider ABI 41 -> 42, 113 ->
114 exports). The prelude is per unit with internal linkage, so a prelude static would be one table
per unit; the archive is linked once per process. The prelude owns the layout
(`kiln_shared_state` in `runtime/heap.c`), stamps it on first use and aborts if two units from
different preludes disagree. Each unit caches the pointer in `kiln_c99_shared_block` (Process).
It holds: a spin lock (writers only), the live-heap count and heap serial counter, the published
shared key table, the process-wide empty key, and the root shape.

**Step 1, keys.** Every intern/lookup (`kiln_c99_intern_key`, `_interned_key_in`, `_intern_static_key`,
new `_intern_pinned_key`) probes the shared table first, then the heap's. Readers are lock-free
(acquire loads of the table pointer and each entry's string; a grown table is built whole, published
with one release store, and the old one is left in place -- bounded, tables only double). Inserts
take the lock.

*Deviation from ADR 0071's "built before the first realm":* the shared table is filled lazily, as
units intern their constants at first entry, because nothing registers a program's constants before
`kiln_realm_create` today and the prelude's own literal keys are scattered. Lazy insertion is only
sound with a rule: a constant joins the shared table only while the interning heap is the only live
heap in the process and does not already hold the spelling as its own key; otherwise it is pinned in
the realm's table exactly as before. That keeps the invariant the lookup order needs -- a spelling is
never a shared key and a live realm's own key at once. Consequence: while two realms are alive, a
constant first seen by either stays realm-local (correct, just unshared). Registering every unit's
constants eagerly from the launcher before the first realm would remove that limitation and let
the rule go; it is an emitter change left for later.

Symbol keys (`@@symbol:N`, registry keys) now use `kiln_c99_intern_pinned_key`: realm-local,
never shared (a realm makes unboundedly many). `KILN_KEYS_CHECK=1` unchanged; a native smoke
program run under it matched the interpreter.

**Step 2, shapes.** `kiln_c99_root_shape` is gone; the root is `&kiln_c99_process()->root`.
`kiln_shape` gains `shared`, `retained`, `owned_next`.
- Shared parent + shared key -> shared child, created and linked under the lock (re-scan under the
  lock; child complete before the edge store; edge count stored last with release; readers load the
  count and list head with acquire), never freed.
- Anything else -> realm-local child owned by the heap (`heap->local_shapes`). A local parent keeps
  the edge in itself. A local edge out of a *shared* parent goes in the heap's side table
  (`heap->local_edges`, keyed by parent, key and the double bit), never in the shared shape, so no
  realm can follow an edge into another realm's memory or match another realm's key.
- Fan-out ceiling (32, class brands to 1024) counts shared edges plus this realm's local edges from
  that parent: the same total as before for one realm.
- Teardown (`kiln_c99_gc_free_local_shapes`) frees local shapes, their edge lists and the side
  table. A teardown survivor (a result read after `kiln_realm_destroy`) marks its local shape chain
  retained during the orphaning sweep; retained shapes are leaked with their edges cleared.
- `node.c`'s request/response layouts are built by ordinary (unbounded) transitions from the root,
  so they are shared shapes when their keys are; the per-thread layout cache is validated by a
  never-reused heap serial instead of the heap pointer (the address-reuse hazard).

**Verified.** `mbx test -p kiln-backend-c --offline`: 176 passed, 1 ignored; emitted_c 36 passed
(ABI surface snapshot updated). New test
`two_heaps_share_constant_keys_and_shapes_but_not_run_time_ones` grows the tree from two heaps in
turn on one thread (fixture hosts the heaps and bumps the realm generation): constant keys and the
shapes built from them are identical pointers in both; run-time keys differ; no shared shape's own
edge names either realm's run-time key; the other realm's local edge is invisible; a second-realm
constant stays local while both live and a new one is shared once alone; teardown of one heap frees
its shapes and leaves the other's intact. It was mutation-checked (sharing every child fails it).
`scripts/native.sh`: 222 matched, 1 refused (shadow-realm, pre-existing), 0 diverged.
`mise run native-stress`: 222 matched, 1 refused, 0 diverged. Provider ABI registry test passes.
A real two-realm test with the provider was not feasible from existing helpers (provider tests have
no prelude; backend tests link a fixture), hence the C-level two-heap test.

**Benchmarks** (native-release, before = 036d288d, alternated; the machine was shared, load 5-29):
cpu and non-io medians within noise on the quieter passes (e.g. json 86 vs 85 ms, regex 5.6 vs 5.7,
parallel-map 78 vs 77, class-poly 1.97 vs 2.0 best-case); swings of +-20-160% appeared only in
passes where load spiked and reversed on rerun. Small service (`bench/server/app.mjs`, mixed,
c=1/16): pairs 6.4k/7.7k, 7.9k/1.9k (load spike), 3.0k/3.1k rps at c=1 before/after; RSS 18.2-18.5
MB after vs 18.8-19.6 MB before. No regression visible above the noise; a quiet-machine rerun is
still worth doing before landing.

**Left for steps 3-5 / risks.**
- Emitted caches (PIC entries, `lit_shape`, receiver rows, typed-callee `_shape`) are process
  statics. Before this change shapes were never freed; now a realm-local shape is freed at teardown,
  so a cache that learned one in realm 1 can false-hit a new local shape at the same address in a
  later realm of the same process. Shared shapes (all-constant layouts) are unaffected. Only matters
  when one unit runs in two realms in one process, which step 3/5 must address anyway (the same
  caches are already wrong across realms for other reasons -- issue 01, class b).
- Unit key statics (`{sym}_key_*`) are still per unit, not per realm; a key that ended up
  realm-local is reused by a second concurrent realm entering the same unit (step 5).
- Retired shared key tables are never freed (step 4's quiescent reclamation can take them).
