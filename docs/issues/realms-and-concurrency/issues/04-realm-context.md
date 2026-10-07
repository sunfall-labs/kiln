# 04. All realm state reachable from the realm

Status: resolved (steps 1, 1b, 2, 3, 3b, 3c, 4 and 5)
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
3b. **Shapes carry attributes, so prototypes have shapes.** Step 3 made method caches correct across
   realms but slowed method calls (class-poly about 1.9x): a class prototype's methods are
   non-enumerable, shapes describe only enumerable data properties, so prototypes live in list
   mode, and a realm-independent method cache must load the closure and check its code on every
   call; list-mode steps also fall back to a per-realm `layout_token`, which ping-pongs between
   realms sharing a site. Give shapes property attributes (writable, enumerable, configurable,
   accessor) so class prototypes and other attribute-bearing objects are shaped, then state method
   caches as receiver shape -> prototype shape -> slot, checking the loaded closure's code.
   Acceptance: class-poly back to its pre-step-3 time or better; `layout_token` gone or unused for
   class prototypes; `scripts/native.sh`, `native-stress` and thirdparty unchanged.
3c. **Shapes record a method's code.** After 3b a method call through a shaped prototype still
   loads the prototype, checks its shape, loads the slot and checks the closure's code: 97
   instructions per call against 79 before step 3. A shape transition that defines a method records
   the method's code (constant-method tracking, as other engines track constant fields), and a
   store that replaces it transitions to a shape without the record. A call site can then check
   receiver and prototype shapes and call the known code directly, loading only what it needs from
   the closure. Acceptance: class-poly at or below the pre-step-3 2.05 ms.
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

### 2026-09-29 -- quiet shared-keys/shapes performance audit

The isolated `036d288d` / `8b0dc1e2` pair shows overlapping class-poly medians,
a consistent JSON increase from 76 to 80 ms, and roughly 1% lower HTTP throughput.
The same JSON and HTTP workloads paired against current `cdecff41` have no overall
regression: JSON takes 62 ms versus 75 ms before sharing; hello and mixed throughput
differ by less than 0.3%, with CPU/request about 3% lower. The current comparison includes
subsequent changes and does not attribute their benefit to sharing. Class-poly's separate
current captured-array audit remains the evidence for that workload.

All 42 expected paired cells have matching checksums or validated response bodies and
statuses. The historical object-spread compiler failure is retained explicitly, without a
timing. Exact records, artifact hashes and proof limits are in `../performance-audit.md`.
Key-registration large-bundle startup and TypeScript clone/bootstrap still need their
separate historical checks.

### 2026-09-26 -- steps 1 and 2 done (branch `work/shared-keys-shapes`, commit 8b0dc1e2)

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

### 2026-09-26 -- step 1b done (branch `work/register-constant-keys`)

**Where registration happens: load-time constructors, not the launcher.** Every emitted unit
with a key table now carries `__attribute__((constructor)) static void {sym}_register_keys(void)`
beside it, and the prelude carries one more, `kiln_c99_register_builtin_keys` (end of
`runtime/surface.c`). Constructors run before `main`, and a shared library's run when it is
loaded, so the linker's own list of linked objects is the enumeration: the graph launcher, the
scalar and embedding entry points, the shared-library wrapper and a chunked unit all get it with no
launcher change and no unit list. The launcher does know its units (the namespaces), but a
launcher-driven call would have had to be added to every entry point separately and would miss an
embedding host that calls `kiln_realm_create` itself.

**Cache keys.** A unit's constructor names only that unit's own constants (a file-scope
`{sym}_key_sources[N]` array of its `{sym}_string_*` headers), so the generated C is still a
function of the unit alone; the only cache effect is the one-time miss from the emitter and
prelude changing. Nothing is added to the launcher, so its key is unchanged.

**What registration does** (`kiln_c99_register_static_keys(keys, out, count)`, `heap.c`): under
the process lock, and only while `live_heaps == 0`, each key is probed/inserted into the shared
table and its shared pointer written to `out`. `ku` passes `ku_keys` straight in; any other symbol
fills its `{sym}_key_*` statics from a stack array. When every key registered, the unit sets
`{sym}_keys_ready = 1` and a new `{sym}_keys_shared = 1`; the realm reset then leaves the
pointers alone (`if (phase == RESET && !{sym}_keys_shared) keys_ready = 0`), so the per-realm
re-intern at first entry disappears. Called with a heap already live (a library loaded into a
running process) it returns 0 and changes nothing: it cannot vouch that no live realm owns the
spelling. Out of memory also returns 0. Either way the unit falls back to the old per-realm
`{sym}_intern_keys` path.

**Built-ins' keys**, registered once per process (`builtins_registered` in the shared block, so
the first prelude copy does it and later copies skip): the per-request hot keys, every string name
in the surface table (258 rows; `@@` names skipped: they are reserved slots reached through a
Symbol), the `node:http` request/response layout names plus `@@node:events` (moved to file scope
in `node.c`; they were already shared lazily in the single-realm case), and a short X-macro list
of keys the object-building paths spell (`value done writable enumerable configurable get set
length name prototype constructor message stack cause errors code`). The method-code table is
deliberately not registered: 2.7k codes through a linear surface lookup would be a measurable
pre-`main` cost, and a method a program reaches by name is already a constant of the unit that
names it.

**The lazy rule is kept, as a fallback only.** `kiln_c99_intern_static_key` still shares an
unregistered constant only while its heap is the only live one. It now covers only (a) prelude
literals outside the registered sets (e.g. `"module completion"`), and (b) units whose
registration came too late or ran out of memory. Every constant of a unit linked into the program
is shared regardless of how many realms are alive. Removing the rule would make (a) and (b)
realm-local always, which is correct but loses sharing in the one-realm case for no gain.

**Empty key.** Registration maps `""` to the process's `empty_key` (no table entry), so a unit's
`""` key static is the process pointer and `kiln_c99_key_is_shared` answers yes for it -- the same
answer the intern paths give.

**Step 5.** The per-unit key statics stay process statics; once shared they are realm-independent
by construction, so step 5 may leave shared unit keys where they are and move only the fallback
(realm-local) case into the per-realm block -- or move all of them; nothing here depends on the
storage location, only on `{sym}_keys_shared`.

**Verified.** `mbx test -p kiln-backend-c --offline`: 176 passed, 1 ignored; emitted_c 36 passed
(four snapshots updated: three units gain the constructor, the ABI surface gains the new prelude
functions). `mbx test -p kiln-native --offline`: 217 passed, 2 ignored; drift 4 passed.
`scripts/native.sh`: 224 matched, 1 refused (shadow-realm, pre-existing), 0 diverged. `mise run
native-stress`: 224 matched, 1 refused, 0 diverged. The two-heap test now emits a unit naming
`gamma` and checks, before any heap, that the constructor filled `{sym}_key_0` with a shared
pointer and set `keys_shared`/`keys_ready`; that built-in names (`statusCode`, `value`, `done`)
intern shared; that a registration after a heap exists refuses; that `gamma` first interned
while two heaps are live is the shared key and the unit's own pointer, with a shared shape edge
both realms follow; and that `epsilon`, which no unit names, still takes the fallback rule
(realm-local while two heaps live). `compact_module_with_many_keys...` checks the `ku` path is
shared before the unit ever runs.

**Performance note.** Hot paths are unchanged (the entry prologue's `keys_ready` check is the
same; reset gained one flag test). Startup gains the constructors: a hash, probe and malloc per
unit constant, formerly paid at each unit's first entry into each realm (plus a heap copy), now
paid once before `main` -- also for units never entered, and ~300 built-in names. Not
re-benchmarked; worth watching in the quiet-machine pass for very large bundles' start time.

### 2026-09-26 -- step 3 done (branch `work/caches-restated`, commit fb335976)

**Entries.** `kiln_c99_pic_entry` is now `shape, transition_to, slot, proto_slot, depth, absent,
level_token[2], level_shape[2], key` (64 bytes, was ~112): no `holder`, `via`, `holder_value`,
`holder_val` or `epoch`. An own property is `slot`. Anything else is a *path* of `depth` (1-2)
prototype steps from the receiver; each level is named by its shape, a list-mode level by a
layout token, and a list-mode *holder* by the key its record carries (`key`, the site's
canonical pointer). A hit walks the receiver's live links, checks each level, reads the slot or
list record live (accessor bit checked). `absent` states "not on the receiver or levels 1..depth,
and the last level's prototype is null -- or undefined while this realm has not modified
Object.prototype"; the same statement is what a transitioning store's clearance now caches
(replacing `via` + epoch). A list-mode *intermediate* is new: the old walk stopped there, so
`b.m()` through `B.prototype -> A.prototype` was never cached before and is now (a probe went
397 -> 104 ms).

**Layout tokens.** `kiln_object.cache_holder` (u8) became `layout_token` (u32, in the same
padding). A list-mode object a path relies on gets a process-unique token from the shared block
(`next_layout_token`, never reused, saturating to "don't cache"); `kiln_c99_prototype_touched`
clears it -- the same mutation paths that used to move the epoch. A token never false-matches
another realm or a reused address, but it is one object's number, so an entry through a
list-mode intermediate or absent-proof level only *hits* in the realm that owns that object;
another realm relearns (correct, not shared). Holders are key-checked and so are shared.
`KILN_IC_CHECK` now traps when a token-trusted list level has gained the key.

**Typed callees.** `_fn`, `_proto`, `_epoch`, `_upvalues` are gone, with the shape-first arm that
called straight from `_shape` without loading the closure. A candidate is recognised by
`plain->body == fn_i` on the closure the site just read live; upvalues are `plain->upvalues`.
`_shape`/`_row` stay (shape-only; rows exist only for shared shapes), and when the receiver's
shape is the remembered one the row is passed in known.

**Built-ins.** A site stores `cached_builtin` only (no `cached_epoch`/`cached_generation`). The
heap carries `modified_builtins` (a bitmap, plus a flag for ids >= 128), marked by
`kiln_c99_callable_store(create)`; a hit asks `kiln_c99_realm_builtin_modified(id)`, which also
counts Object/Function/%TypedArray%.prototype, only when `kiln_c99_builtin_store_epoch` is
nonzero. That word stays, reclassified Process: "some realm modified some built-in", never
stored in an entry; the emitted `== 0` speculations keep reading it (conservative for every
realm). `kiln_c99_proto_epoch` is deleted.

**Shapes and teardown.** `lit_shape`, `kiln_sp_shape_*`, receiver rows (`kiln_c99_recv_claim`)
and private-field sites cache only `shape->shared` shapes -- installed/used without comparison
or through process statics, they must never see a freeable address. Sites (`kiln_c99_site`) may
hold realm-local shapes and keys (JSON-parsed objects with unnamed keys stay cached):
`kiln_c99_gc_register_site` now registers a site only when an entry names a realm-local shape
or key (`site->registered` is the heap serial), and teardown calls
`kiln_c99_gc_forget_realm_local` *before* sweeping, dropping exactly those entries. The GC-time
dead-holder sweep is gone (nothing in an entry is a heap object); `heap->sites` is now freed.

**Verified.** `mbx test -p kiln-backend-c --offline`: 208 passed, 1 ignored; emitted_c 36 passed
(snapshots updated). New test `a_site_forgets_a_torn_down_realms_local_shapes_and_keys` (two
heaps: site registers only for local entries, forgets them at teardown and keeps the shared one;
per-realm built-in record; token issue/clear/no-reuse). `scripts/native.sh`: 234 matched,
1 refused, 0 diverged. `mise run native-stress`: 234 matched, 1 refused, 0 diverged.
`scripts/thirdparty.sh`: 17 matched, 0 diverged (5 bundles not built); native 17/0/0.
Hand probes (reassigned method, shadowing on the way, swapped prototype, same-shape classes,
absent then gained, Object.prototype gained, accessor replacing a method, setter chain on a
transitioning store, `Math.max` patched) agree with node and the interpreter, also with
`KILN_IC_CHECK=1`.

**Benchmarks** (native-release, before = fa213915, alternated; load average 21-24 throughout, so
treat single numbers with suspicion). class-poly warm median: before 2.0 ms best / after 4.06
(quiet passes), 4.8 / 10.6 under load -- a real ~1.9x regression: the removed shape-first typed
arm never loaded the callee; ADR 0071 requires loading it and checking its code, and the path
walk plus closure load is ~3-4 ns per call (a standalone copy: 79 -> 117 ms min of 15). An
entry-index variant that skipped the entry search bought only ~3% and was dropped.
collections: 17.25 / 16.5 ms min, 22.3 / 21.3 median (no change). json: noise-dominated
(152 / 127 ms first pass). Small service (`app.mjs`, drive-mix, c=16, 5 alternating 4 s passes):
6945 / 6904 rps median, server CPU 73.3 / 75.5 us per request (~3%, inside this machine's
spread today).

**Left / risks.**
- class-poly-style typed dispatch through a class prototype is ~2x slower. Recovering it
  realm-independently needs class prototypes to have shapes (so a hit is shape compares plus
  one slot load) -- a shapes change, not a cache change.
- Token-named entries (list-mode intermediates, absent/clearance through class prototypes) are
  correct everywhere but per realm in effect; two live realms sharing such a site ping-pong.
- A cached built-in member of a realm that has ever written Object/Function/%TypedArray%.prototype
  stays uncached in that realm (sticky marks; the old epoch relearned).
- Step 4 is untouched: entries are written non-atomically; the teardown sweep writes sites
  another realm's thread could be reading once realms run concurrently.

### 2026-09-26 -- step 3b done (branch `work/shapes-attributes`)

**Shapes carry attributes.** `kiln_shape` gains `attrs[32]` (per slot, `KILN_C99_ATTR_ENUM_FLIP |
READONLY | FIXED | ACCESSOR`, stated as differences from what an assignment creates, so every
shape an assignment builds is all zeros and unchanged) and three summary masks: `special_mask`
(anything but a plain assignment's property), `accessor_mask` (the slot holds an accessor pair)
and `store_mask` (an assignment may not simply overwrite: read-only or accessor). A transition is
keyed by (key, double bit, attributes): every edge match -- inline edges, the list, the edge
index table, the realm-local side table -- compares the attributes of the slot the child adds
(`kiln_c99_shape_edge_is`; an assignment's edge is answered from the special bit). Sharing is
exactly steps 1-2's rule (shared parent + shared key -> shared child, never freed; otherwise
realm-local), so attributes add no new sharing case. An accessor keeps its getter/setter in its
one slot as a pair (a two-element array no program sees). `kiln_c99_ensure_dictionary`
materializes exactly the records the list path would have built (enumerable = !reserved xor
flip, writable, configurable, accessor halves), so every fallback is the old behaviour.

**What is now shaped.** Anything defined, rather than assigned, on a shaped object:
`kiln_c99_define_method_key` (class methods, `constructor` on every `prototype_object`, the
runtime's `kiln_c99_define_method` objects -- console, process, node stream/emitter prototypes),
`kiln_c99_define_accessor_key` (class and literal getters/setters, the second half merged into
the pair in place when that leaves the attributes unchanged), and private methods
(`define_private*`, read-only). Only what the list path would do without asking anyone is taken
shaped: a new key on an ordinary extensible object whose chain would not intercept the list
path's assignment, or an existing slot already carrying exactly those attributes. Everything else
-- index-like keys, frozen/non-extensible/namespace objects, redefinitions that change
attributes, `defineProperty`, delete, freeze, objects past 32 properties -- takes the unchanged
list path (dictionary mode).

**Every slot path audited.** A by-key slot read checks `accessor_mask` (the get walk calls the
getter from the pair; site learners never learn an accessor slot; receiver rows refuse one; node
layout loads), a by-key slot write checks `store_mask` (the put fast path runs the setter or
refuses a read-only slot in place; site learners never learn a read-only slot; private-field
stores; node layout stores), and the prototype-chain write walk (`kiln_c99_level_write_answer`)
answers from a shaped level's attributes instead of converting it to a dictionary. Walks that
read slots as enumerable data honour the attributes (own keys, for-in, JSON, Object.keys/values/
entries) or leave shape mode first when the shape has special slots (spread, sunk spread,
Object.assign, the structured clone). Console inspection reads a shaped prototype's
`constructor` slot instead of converting the prototype. A write site only remembers a transition
that is the single edge its own key took (a setter that ran could have moved the object along
another key).

**Method caches.** A site entry that is a shaped holder one step up is marked `one_step` by
`kiln_c99_site_remember`: receiver shape -> prototype shape -> slot, read live, no walk. A typed
call site additionally keeps, per candidate, a `kiln_c99_method_path` (receiver shape, prototype
shape, slot; both shapes shared, never an object), taught from the entry that just answered and
checked before the site's entries: receiver shape compare, prototype shape compare, one slot
load, `plain->body == fn_i`, then the typed call with the known row. The list-mode holder read
moved out of line (`kiln_c99_pic_list_holder`), which keeps inlined site code smaller than
main's (text -134 KB for class-poly, -151 KB for the small service).

**`layout_token`.** Kept, but only dictionary-mode levels get one: class prototypes are shaped,
so none is issued for them. What remains token-named is an intermediate or absent-proof level in
dictionary mode (past the shape limits, deleted from, redefined, frozen) -- genuinely list mode,
with no layout to name.

**Verified** (rebased onto origin/main d2c0e686, after the objmodel miss/literal work; its shape
edge index now matches on attributes too). `mbx test -p kiln-backend-c --offline`: 178 passed, 1
ignored; emitted_c 36 passed (ABI surface snapshot re-recorded by `INSTA_UPDATE=always`; prelude
size band widened to 51,800 at 51,549 lines, +464). New test
`definitions_keep_objects_shaped_with_attributes` (method/assignment edges differ, same
definitions share a shape, accessor pair and private read-only masks, one_step entry reads live
and misses once the prototype leaves shape mode, accessor slot never a path, dictionary
records). `scripts/native.sh`: 243 matched, 1 refused, 0 diverged. `mise run native-stress` over
the 229 non-`parallel-*` probes: 229 matched, 1 refused, 0 diverged -- the fourteen `parallel-*`
probes under `KILN_GC_STRESS=1` run for tens of minutes each on origin/main too
(`parallel-refused.js` timed out at 10 min with main's binary), so they were left out of the
stress run; all of them matched in the ordinary run. `scripts/thirdparty.sh`: 17 matched,
0 diverged, 5 not built; native 17/0/0. Hand probes (methods, getters/setters on prototypes and
literals, private methods/accessors, reassigning and deleting a method, shadowing, swapped
prototypes, defineProperty on a prototype, a frozen prototype, 34-method and 40-method
prototypes, for-in/keys/entries/JSON/spread/assign of instances, console.log) agree with node
and the interpreter, also with `KILN_IC_CHECK=1` and `KILN_GC_STRESS=1`.

**Benchmarks** (native-release, alternated; load average 9-11 for these passes, other passes
earlier in the day at 20-50 were discarded). class-poly warm median over 16 rounds: pre-step-3
(fa213915) 2.05 ms, step 3 (origin/main) 4.66 ms, step 3b 2.94 ms (best 2.00 / 4.00 / 2.75).
Load-free counts on a fixed-iteration copy (50M calls): 79 / 149 / 97 instructions per call,
30 / 63-74 / 42-44 cycles. collections and json (instructions, fixed iterations): +0.5% and
+0.3% over main; wall clock best-of-9 8.0 / 7.9 ms and 88 / 90 ms, medians noise-dominated.
Small service (app.mjs, drive-mix, c=16, 7 alternating 4 s passes): 60.1 / 59.9 us server CPU
per request, 10320 / 10325 rps median (main / 3b); 101.5k / 101.4k instructions per request.

**Left / risks.**
- class-poly is ~1.4x pre-step-3, not back to it: the prototype load, its shape check, the slot
  load and the closure's code check are a four-load chain the old shape-only arm never paid. What
  would remove it (a shape stating the slot's code) is a constant-field scheme, not attempted.
- A method path is monomorphic per candidate (one receiver shape): several receiver shapes
  reaching one candidate through one step relearn it (cheap), and a candidate found two steps up
  takes the site's entries as before.
- Paths that read or convert shapes by key outside the audited ones fall back through
  `kiln_c99_ensure_dictionary`, correct but leaving shape mode for good: `getOwnPropertyDescriptor`,
  `getOwnPropertyNames`, Reflect/proxy receivers, a function's static store, an Array subclass's
  store walk. A program that does one of those to a class prototype is back to step 3's speed.
- Pre-existing, unchanged: an enumerable symbol accessor from a literal
  (`{ get [Symbol.toStringTag]() {} }`) is listed by `Object.keys` as `@@toStringTag` natively
  (node lists nothing); the list path did the same before this change.
- Step 4 is still untouched: method paths, like the rows beside them, are several words written
  non-atomically.

### 2026-09-26 -- step 5 done (branch `work/realm-context`)

Rebased onto 61fddb43 (steps 3b and 3c) before the final runs.

**The realm context hangs off the heap.** `kiln_gc_heap` gains `kiln_c99_realm_state realm`
(`runtime/foundation.c`), so the one per-realm object every unit already finds through the
generation-checked `kiln_c99_gc_heap` is the context; `kiln_c99_realm()` answers it (the fallback
heap's outside a realm, as the statics were one per process). It holds the symbol registry,
`node:ffi` function and callback metadata and the next callback-root key, `Array.prototype
[Symbol.unscopables]`, every native constructor's prototype (the constructor table is now `const`;
the prototypes are an array at the same index), and every compiled unit's module block. The heap
marks all of it in `kiln_c99_gc_collect` (`kiln_c99_gc_mark_realm`) instead of relying on some
unit having registered a root function, and `kiln_c99_gc_teardown_end` frees it
(`kiln_c99_realm_free`). `kiln_c99_gc_unit_roots` now only marks and resets the frame registers.

**Per-realm, per-unit module blocks.** A unit's module globals and template sites are one array of
`kiln_val` in a block the realm allocates on its first entry into the unit (`kiln_c99_unit_vals_slow`,
`heap.c`). Each unit carries a descriptor `{sym}_unit = { id, val_count }`; the id is its row in every
realm's `units[]` table, assigned once per process from `kiln_shared_state.next_unit_id` the first
time any realm asks (compare-and-swap, so concurrent first entries agree). A per-thread
`{sym}_unit_cached` holds `{ generation slot, generation, vals }` exactly as `kiln_c99_realm_cached`
holds the heap: the fast path is two loads and a compare. `{sym}_globals` and `{sym}_template_sites`
are now macros over `{sym}_unit_vals()`, so every emission site kept its spelling. A function body
that reads or writes its module's globals asks once at entry (`kiln_val *const kiln_mg = ...;`,
`emit_graph::body::cache_module_globals`) and indexes `kiln_mg[slot]`; a resumed generator or async
body re-enters from the top and asks again. The module entry already cached `locals =
{sym}_globals`. Chunked units: the descriptor is external, each chunk has its own cache. Parallel
clones that would read `kiln_mg` are refused (a pool thread has no realm generation).
`{sym}_gc_roots` no longer marks globals or template sites (the heap does); it keeps only the
fallback-key reset.

**The graph launcher's linkage is its realm's.** `kiln_exports_*`, `kiln_imports_*`,
`kiln_namespaces`, `kiln_opentui_namespace`, `kiln_node_ffi_namespace` and `kiln_commonjs_*` are
fields of `kiln_graph_context`, which `kiln_cli_run_realm` now `calloc`s per realm run (its result
slots are read after teardown, so it is freed after `kiln_realm_destroy`). The old names are macros
over `graph->...`; every launcher function that uses them has the graph -- from its argument, the
host's context (dynamic import), or the CommonJS require context, which the wrapper is now handed
instead of `NULL`. The roots function reads the graph it was registered with. The shared-library
wrapper already kept its graph in a realm context.

**Launcher buffers and last error.** `kiln_cli_last_host_error` and both `static kiln_string header`
buffers are gone. `kiln_cli_host_state { last_error, error_text }` (declared by
`kiln_cli_host_state_declaration!`, in the embedding header) is per realm: the scalar launcher keeps
one in `main` and passes it as the host context; the graph launcher keeps one in its graph and
passes it from `kiln_graph_host_call`. `kiln_cli_report_failure_with(..., host_state)` reads the
realm's; a caller passing no context gets the calling thread's. `kiln_graph_diagnostic`'s scratch is
per thread.

**Rust.** `node:ffi` `State` (libraries, prepared functions, callback closures) is a field of
`NativeRealm` and is dropped at the start of the realm's teardown, before the heap its callbacks'
userdata points into; handles are numbered per realm. Calls outside any realm (the crate's own
tests) use a thread-local state. `LAST_ERROR` stays per thread (an out-parameter). The active-realm
registry (`REALMS`, `ACTIVE_*`, `ACTIVE_GENERATION`) was already per thread and needs nothing: a
thread runs one realm at a time, and each thread has its own generation word.

**Ownership manifests.** `C99_MUTABLE_STATE_OWNERS` has no `Realm` owner any more: realm state is
not a static. Owners are `Frame` and `ThreadCache` (both must be declared `_Thread_local`, which the
test now checks), `Process`, `Constant`, `TestFixture`. The emitters have a manifest of their own,
`kiln_backend_c::state_owners::EMITTED_MUTABLE_STATE_OWNERS`, keyed by normalized name (`{sym}`,
`N` for digit runs), with owners `Cache` (ADR 0071 caches), `ThreadCache`, `Process`, `Constant`.
`kiln-native`'s `every_mutable_emitted_static_has_an_owner_that_is_not_a_realm` compiles a program
that reaches every static-declaring emission path (module globals from functions, templates,
classes and typed methods, literals and spreads, private fields, BigInt, regexp, loops, generators,
async, dynamic import, CommonJS) under three policies (default, compact, generic-only), and checks
every unit, the graph launcher and the shared-library wrapper: a mutable static with no row -- the
old `static kiln_val {sym}_globals[N]` or `static kiln_value kiln_namespaces[N]` has none -- fails,
and so does a per-thread row declared without `_Thread_local`. `state_owners` has a unit test that
the old realm-state declarations are refused.

**What is per thread, not per realm, and why (a deliberate reading of ADR 0071's list).** The frame
registers (`kiln_c99_new_target`, `_inheriting_new_target`, `_typed_status/_thrown`,
`_adopted_this`, `_frame_*`, `_captured_arguments`), the `node:ffi` callback in flight, the HTTP
response being written, and the refusal diagnostic (`kiln_c99_diagnostic_*`, now fields of one
`_Thread_local kiln_c99_diagnostic`, and `_active_depth`) are `_Thread_local`. They are a spill of
the running call's stack: saved and restored around every call, idle at every turn boundary, and a
thread runs one realm at a time (the provider refuses a nested enter), so what a thread's registers
hold always belongs to the realm it is running. In the realm context they would cost a generation
check on every call's save and restore; as thread-locals an executable's `%fs` access costs what a
static did. The diagnostic in particular cannot be the realm's: the launcher reads it after the
realm has exited (`kiln_c99_control_diagnostic` outside `kiln_realm_is_active`), which the first
per-realm version of this change broke (four `native_non_callable_*` tests caught it); the realm's
own record is still `kiln_realm_diagnostic_note`. The recycled typed-array byte blocks
(`kiln_c99_bytes_cache`) are per thread too: plain memory naming no realm's value, retired from a
sweep -- including the teardown sweep, where no realm is current. `kiln_graph_diagnostic`'s scratch
is per thread for the same reason as the diagnostic.

**What is still process-wide.** The inline caches (ADR 0071: layouts, paths, code), including step
3b's method paths; the unit and launcher speculation/profile counters; a unit's descriptor (its id)
and its constant keys, which step 1b registers before any realm. The one realm-dependent leftover is
a unit whose keys could not be registered (loaded after a realm existed): it keeps the per-realm
re-intern at first entry and reset at teardown, which is right for realms in turn and wrong for two
such realms running at once -- issue 05's, with `kiln_c99_builtin_store_epoch`, which the graph
launcher still zeroes after each realm's bootstrap (process-wide: a concurrent realm that modified a
built-in would lose its conservative answer).

**Verified** (on the rebased tree). `mbx test`: kiln-backend-c 180 passed, 1 ignored, emitted_c 36
(two snapshots updated: `module_scope_slots` shows the unit block and `kiln_mg`, the ABI surface the
new declarations); kiln-native 218 passed, 2 ignored; kiln-native-runtime 16;
kiln-native-provider-ffi 34, and 40 with `--features ffi`. New tests:
`two_realms_running_one_unit_keep_their_own_module_globals` (backend-c: two heaps on one thread run
one unit's body; the second sees an empty block before its own run; a closure the first realm made
still reads the first realm's global after the second stored its own; teardown frees each block;
mutation-checked by pointing the slow path at one shared table, which fails it),
`realms_on_two_threads_are_active_at_once_and_each_sees_only_its_own` and
`ffi_libraries_belong_to_the_realm_that_opened_them` (provider), the two manifest tests above.
`scripts/native.sh`: 243 matched, 1 refused (shadow-realm, pre-existing), 0 diverged.
`KILN_NATIVE_GC_STRESS=1 scripts/native.sh` (`mise run native-stress`): 243 matched, 1 refused,
0 diverged. `scripts/thirdparty.sh`: 17 matched, 0 diverged (5 bundles not built); native 17/0/0.

**Benchmarks** (native-release; before = 61fddb43, after = this change; alternated; load average
9-26 throughout, so medians are noise-dominated and best-of values are quoted). fib warm best 19.25
/ 21.0 ms with unchanged machine code for `fib_t` (same instructions, byte for byte): the function
moved from a 32-byte to a 16-byte aligned address; rebuilt with `-falign-functions=64` both give
18.0-18.25 ms. frame (arguments, closures: the frame registers) aligned builds, six alternating
passes at load 25: medians 12.4k / 12.2k ns, best 10.0k / 9.3k -- no difference visible.
class-poly 3.25 / 3.25 ms median of medians (best 2.88 / 3.06), collections 7.38 / 7.38 ms, json
79 / 79 ms. Small service (`bench/server/app.mjs`, drive-mix c=16, server pinned to one core, 8
alternating 4 s passes): on the three quietest passes 41.4 / 40.9, 43.0 / 40.9, 40.9 / 42.4 us of
server CPU per request (before / after); RSS 18.3-20.1 MB both.

**Left / risks.**
- Step 4 (atomic publication and reclamation) is untouched.
- Realms on several threads at once (issue 05) additionally need: the fallback-key units and
  `kiln_c99_builtin_store_epoch` above; `KILN_WORKERS` still forks.
- A function body that touches its module's globals now pays the block lookup (two TLS loads and a
  compare) once per entry, before its first block; bodies that do not touch them pay nothing.
- A unit id is never reused and each realm's table is indexed by it, so a process that loads
  thousands of units carries a table that long per realm (8 bytes a row); blocks are made only for
  units a realm enters. Out of memory making a block aborts with a message (the static it replaces
  could not fail).
- The frame registers and diagnostic being per thread relies on a realm never holding a generated
  frame across a turn boundary (ADR 0041), which ADR 0070's scheduler must keep true.
### 2026-09-26 -- step 3c done (branch `work/shapes-method-code`, commits 5fdd60e8, cf061050)

**Shapes record a method's code.** `kiln_shape` gains `code` (the code -- `kiln_closure.body` --
of the slot this shape adds, NULL when none) and `code_mask` (the slots with a record). A record
is made only by a method *definition*: `kiln_c99_define_shaped` kind 0 (class methods,
`kiln_c99_define_method` objects) when the value is a closure and the key is not `constructor`
(every prototype defines it first; a record there would give the root one branch per class for
nothing a call site reads). Private methods and accessors record nothing. A record is code, never
a closure, so shapes stay realm-independent and sharing is steps 1-2's rule unchanged.

The code is part of the transition's key beside the attributes (`kiln_c99_shape_edge_is` compares
`child->code` for attribute-bearing slots; every edge match -- inline, list, edge index, the
realm-local side table -- goes through it). Two closures of one code (two realms, or a class made
by a factory twice) define one shape; two codes define two. A branch with no room for the recorded
edge takes the unrecorded one. Only attribute-bearing slots carry records, so an assignment's edge
still matches from the special bit.

**The record is always true.** A recorded slot is in `store_mask`, so no assignment fast path
(site entries, `ku_pic_poly_write`, boxed/node layout stores, private sites) writes it; the site
learners already refuse `store_mask` slots. The put path (`kiln_c99_put_mode_key`) sends it to
`kiln_c99_shape_store_recorded`: a closure of the same code is stored in place; anything else
(another function, a non-function) first moves the object to its own shape with that slot's record
dropped -- rebuilt from the slot's parent by ordinary transitions with the same keys, attributes,
double bits and every other record -- or, if a branch is full, to dictionary mode. A redefinition
in place (`define_shaped`, same attributes) takes the same route. delete, `defineProperty`, freeze
and the other attribute changes already leave shape mode. Object-literal methods are not recorded:
they lower to `DefineNamedProperty` (an enumerable data definition, the same edge as an
assignment), and `method_candidates` only ever names `DefineMethod` functions, so no typed site
could read a literal's record.

**The call.** A typed site's per-candidate `kiln_c99_method_path` (receiver shape, prototype
shape, row, slot) is learned only from a one-step shaped entry whose prototype shape records the
candidate's code (`kiln_c99_shape_slot_code`), together with the candidate's receiver row. A hit:
receiver shape compare, prototype tag and link, prototype shape compare, the typed guard, then the
candidate's `_t` entry called directly with the path's row -- no closure load, no code check, no
second shape compare for the row. The closure is read (from the live slot) only for a candidate
that takes upvalues, so two closures of one code each supply their own. Paths start as
`KILN_C99_UNLEARNED_METHOD_PATH` (a receiver shape no object has), which drops the "receiver is
shaped" test.

**Layout.** `kiln_object.prototype` moved to directly before `shape`: the hit reads shape, link and
slots, and the link three fields up cost a cache line per call (fixed-iteration class-poly L1 misses
per call: pre-step-3 1.79, step 3b 5.7, 3c 2.89 before the move and 2.06 after). Nothing names
members by offset. The move exposed a latent bug: `kiln_c99_define_private{,_key}` set
`read_only_seen` through `kiln_c99_object_of(target)` on a class (a static private method) and so
wrote into the closure itself; it now marks the class's property store. Before the move the write
landed in a closure field nothing tripped over (`private-constructor-static-call` and
`classes-and-callables` failed "not a constructor" with the new offsets).

**Verified.** `mbx test -p kiln-backend-c --offline`: 179 passed, 1 ignored; emitted_c 36 passed
(ABI surface snapshot re-recorded with `INSTA_UPDATE=always`). New test
`method_definitions_record_their_code_and_stores_keep_it_true` (same code shares a shape, other
code does not, numbers and `constructor` record nothing, a path learns only for the recorded code
and a row when one is needed, an unlearned path matches nothing, a same-code store keeps the shape,
an other-code store and a non-function store move to the unrecorded sibling keeping later records,
the twin prototype keeps its record). `scripts/native.sh`: 243 matched, 1 refused, 0 diverged.
Stress (`KILN_NATIVE_GC_STRESS=1`) over the 229 non-`parallel-*` probes (copied to a scratch
directory; this base has no runner skip): 229 matched, 1 refused, 0 diverged.
`scripts/thirdparty.sh`: 17 matched, 0 diverged, 5 not built; native 17/0/0. Hand probes agree
with node and the interpreter, also with `KILN_IC_CHECK=1` and `KILN_GC_STRESS=1`: same function
stored back, reassigned, restored, non-function (TypeError), deleted (falls to the parent's),
own-property shadowing, `setPrototypeOf` to another class, a getter via `defineProperty` then a
value, `Object.assign` onto a prototype, factory classes with one code and different captured `k`
(also with one class's method replaced mid-loop and replaced by another closure of the same code),
a frozen prototype, a duplicated method in a class body, literal methods, `Object.keys` and
`getOwnPropertyNames`.

**Benchmarks** (native-release, alternated, load 8-15 unless noted; pre-step-3 = fa213915, 3b =
61fddb43). Fixed-iteration class-poly, harness-shaped (50M calls, 21 alternating runs):
instructions per call 79.3 / 97.3 / 79.8, cycles per call median 16.3 / 23.2 / 18.0. Harness
class-poly, 16 rounds, median of warm medians: 2.19 / 3.13 / 2.42 ms (best 2.00 / 2.81 / 2.13).
collections and json at fixed iterations: instructions +0.06% and +0.15% over 3b; wall clock
best-of-11 6.0 / 6.0 / 6.5 ms and 81 / 62 / 63 ms (medians noise-dominated at this load). Small
service (app.mjs, drive-mix, c=16, 7 alternating 4 s passes): server CPU 39.6 / 37.6 / 38.8 us per
request, 14.6k / 14.8k / 14.5k rps median.

**Left / risks.**
- class-poly is at pre-step-3 instructions (+0.5 per call, the prototype tag test) but ~10%
  behind it in cycles: the hit still follows receiver -> prototype -> prototype's shape, a
  dependent load chain the old arm (which trusted a remembered closure) never had. Closing that
  would need the receiver's shape to imply its prototype, which shapes deliberately do not.
- More shapes: class prototypes no longer share shapes by method names; each distinct method code
  is its own branch (bounded by the 1024-branch and 65,536-shape limits, falling back to the
  unrecorded edge).
- The method path is several words written non-atomically, and a torn path could now pair a
  receiver shape with another learn's slot; that matters once realms run concurrently (step 4).
- A shared library unloaded while its shapes live leaves records naming its code; objects holding
  that code die with it, but a later library at the same address could be matched by an old path.

### 2026-09-27 -- step 4 done (branch `work/concurrent-realms`)

**One mechanism: a slot** (`KILN_C99_SLOT_*`, `prelude_types.h`; ADR 0071 amended). Every answer a
shared cache holds is a few words named by one key word, the receiver shape. A reader loads the key
with acquire and reads the rest only after it matches. A writer claims the slot by CAS of the key word
to CLAIMED, writes the words, publishes with a release store of the shape. A published slot never
changes. Replacing one claims it; it is rewritten in place only while no other thread runs a realm
(`kiln_realm_retire_tag` answers 0 -- every single-realm process, so its behaviour is the old in-place
overwrite), otherwise its key word becomes RETIRED(tag) and it is claimed again only once the tag is
safe. No sentinel is a shape (they use the low two bits), so readers need no extra test. Chosen over
records behind a pointer: no dependent load, no allocation, nothing to free; the cost is capacity (a
retired entry is unusable until its grace period ends). On x86-64 the acquire load is spelled as a
volatile load plus a compiler-only acquire fence (`KILN_C99_LOAD_ACQUIRE`), which the compiler folds
into the compare exactly as it did the plain load.

Per structure:
- **PIC entries** (`kiln_c99_site.entries`): slots keyed by shape; `pic_count` counts entries ever
  claimed and only grows. `kiln_c99_site_remember` replaces the same shape's entry (in place when
  alone, else retires it and writes a fresh or safely-retired entry), else "over the last" as before.
  Teardown's forget retires local entries in place (it used to compact the array under readers).
  The transition-clearance path restates the entry through remember instead of editing it. Every
  scan goes through `kiln_c99_site_entry`; the emitters read `entries[i].shape` through
  `kiln_c99_slot_key`. A transition from a shared shape into a realm-local one (a run-time key) is
  never remembered: an entry is *compared* against a receiver's shape but its `transition_to` is
  *given* to it, and another realm running now would be moved into the learning realm's shape and
  key (the realms harness found this: `{ first, ...s, last: "L" }` read `last` as undefined in the
  second realm). `DefineNamedProperty` literal keys now join the unit's key table, so the common case
  is a shared key, a shared child and a cached store as before.
- **MRU swap** (compact `ku_pic_poly_read/write` swapped a hit to entry 0 on every access): removed;
  an entry that answers the out-of-line scan 8 times running moves to the front, only while alone
  (`kiln_c99_site_answered`/`_promote`). The streak is per thread (a TLS table), so counting writes
  no shared line. Round-robin polymorphism no longer swaps per access at all.
- **One-word site fields** (`own`, `proto`, `misses`, the `length` hint): single relaxed words,
  each validated where used; the hint is written only when it changes (a store of the same value
  still takes the line from every other realm's thread).
- **Receiver-kind hints were trusted.** An element or `length` site that had seen an array read the
  next buffer-prefixed receiver as an array without asking -- arrays and typed arrays share the
  prefix -- so `readL([1,2,3]); readL(new Uint8Array(5))` answered 0 (on main too). With realms the
  hint is another realm's to write at any moment (`negative-cache-invalidation`, 16 realms under
  stress: `exotic typed 0 9`). Element sites now ask the block's kind, inline: the header read
  moved to `prelude_types.h` (`kiln_c99_gc_payload_kind`, one byte compare, its offset asserted in
  `heap.c`), and element sites write nothing to the site; the `length` hint only picks which check
  comes first.
- **Cached method** (tag, method, builtin): a one-byte slot 0 / 2 (writing) / 1, words published
  behind a release store; replaced only while alone (the old answer stays true for its kind).
- **Method paths**: one slot keyed by `recv` (UNLEARNED is the free state).
- **Receiver rows**: write-once slots, never retired. Claimed in order with one claim in flight per
  table (a claimer waits out the few instructions another is filling a row): racing realms took a
  row each for one shape, left none for the next shape, and the typed entry then refused (16 realms
  of class-poly: 13 refusals before this).
- **Typed `_shape`/`_row` memo**: removed (two words written separately); the arm looks the row up in
  the callee's write-once table, the same instructions on a hit.
- **Private-field sites**: one slot keyed by shape. **`lit_shape`**: one word, read once
  (acquire), published with release. **Spread slots**: a write-once slot for the first shared shape;
  other shapes work theirs out per execution.
- **Prelude process statics**: the built-in member index and method index are built under
  `kiln_c99_once` (both sorted *in place*, so a racing second builder let a reader search a
  half-sorted table); env-mode words, `1n`/2^64-1 BigInt handles and the method-code bound are single
  words published once; the static-text header table claims each header by CAS; `Symbol()` serials are
  an atomic counter (a lost update could mint one serial twice in one realm); the `KILN_SPECULATE=exit`
  draw is per thread; profile tags use atomic ORs. Function-scope statics now carry `kiln_c99_` names
  so the ownership manifest sees them.
- **Shape counts**: shared shapes are counted in the process block under its lock, local shapes per
  realm on the heap (one per-prelude count let realms use up each other's room).
- **Retired records**: a grown shared key table and a grown shared shape's edge index are handed to
  `kiln_realm_retire` and freed at a later boundary (they used to be leaked).

**Reclamation** (`kiln-native-provider-ffi/src/provider/realm/reclaim.rs`): one epoch and one word per
thread. A thread is online while a realm is active on it (`ActiveRealm`), while a realm is torn down,
and while an HTTP event is delivered into a parked drive; the drive's loop top is a turn boundary
(`quiescent`), and a parked drive is offline so an idle realm holds nobody back. A tag is safe once
every online thread has seen a later epoch. Single-realm cost: a load and a store per turn. Provider
ABI 45 -> 46 on the rebased base (`kiln_realm_retire_tag`, `kiln_realm_retired_safe`, `kiln_realm_retire`, and issue 05's
`kiln_realm_run_pinned`), 122 exports.

**Step 5's hazards.** `kiln_c99_builtin_store_epoch`: the launcher no longer zeroes it; each realm
takes back its bootstrap's stores (`kiln_c99_builtin_bootstrap_done`) and everything else at teardown,
so it counts live realms' non-bootstrap stores. Every emitted built-in speculation asks
`kiln_c99_builtins_unmodified()`: the summary, and when it is nonzero the current realm's own record
(a pure, cold, register-preserving call, so the hot side is unchanged). Before this a typed body
treated any realm's store -- or another realm's bootstrap in progress -- as its own and refused.
Late-loaded units: `{sym}_keys_ready` became `{sym}_keys_owner`; such a unit's key statics hold one
live realm's keys at a time, and a second live realm entering it is refused with a diagnostic rather
than handed the first one's keys (sharing them late would need every live realm's key table, and a
per-realm copy an indirection on every key use).

**Verified** (once, on the final tree). `mbx test`: kiln-backend-c 184 passed, 1 ignored, emitted_c 36
(four snapshots updated; prelude size band widened to 53,100); kiln-native 219 passed, 2 ignored
(+4 drift); kiln-native-provider-ffi 36 (42 with `--features ffi`); kiln-runtime 158;
kiln-native-runtime 16; kiln-native-provider-contract 11+5. New tests:
`a_replaced_answer_is_retired_until_every_reader_has_passed_a_boundary` (the fixture can stand for
other threads), the two `reclaim` domain tests, `pinned_realms_run_at_once_each_on_its_own_counted_thread`.
`scripts/native.sh`: 244 matched, 1 refused (shadow-realm), 0 diverged. Issue 05's harness: see there.

**Instructions** (native-release, fixed iterations, `perf stat -e instructions:u`, base 0da8a1c4):
class-poly-shaped (20M typed method calls through a 1,000-element array) 1,881,991,092 ->
1,881,980,780 (equal); a 4-shape round-robin property read/write loop 5,760,004,695 ->
4,927,308,385 (-14%, no per-access MRU swap); a phase-shaped polymorphic read 1,372,247,955 ->
1,272,223,714 (-7%). Cycles were noise-dominated (load 9-17 from other agents). Two intermediate
versions measured worse and were fixed: plain atomic loads (not folded into the compare, +2 per
class-poly call) and an ordinary call on the unlikely side of the built-in check (+1.5, spills).

**Found, fixed** (pre-existing on main): the typed-array `length` hint above.

**Found, not fixed (pre-existing on main).** A realm that stores onto any built-in makes typed method
bodies that speculate on a built-in refuse fatally: `Math.foo = 1;` before bench/non-io/class-poly
stops with "CallMethod invocation refused" at 0da8a1c4 too. A typed body has no generic fallback to
leave for; that belongs to the speculation design.

**Left / risks.**
- A site relearning many times within one turn while other realms run learns less (retired entries
  wait for their grace period); a single realm never waits.
- Stores through a run-time key from a shared shape are no longer cached (their child is realm-local).
- A thread that reads shared cache state must be online. Generated code only runs inside
  `ActiveRealm`, teardown and HTTP events are bracketed, and `parallelMap` workers touch no caches;
  anything new that calls generated code outside those must bracket itself (`reclaim::online`).
- The streak table and hints keep a polymorphic site's hit path free of shared writes; with 16 realms
  on 16 hardware threads the round-robin loop ran the same instructions per realm and 2.3x the cycles
  (shared L3/SMT, 8 cores; not coherence writes as far as the counters show).
