# ADR 0071: Caches remember layouts, paths and code, never objects

## Status

Accepted, 2026-09-26, under the project owner's delegation of the concurrency and parallelism work
("take on this whole problem"). Revisit if the measurements it calls for contradict it.

Makes ADR 0070 (a realm is a schedulable unit) sound and cheap. Refines ADR 0050 (an object points
at a shape) and ADR 0047 (interned keys). Uses ADR 0041's turn boundary a third time.

## Context

ADR 0070 runs many realms in one process, each moving between threads at turn boundaries. The
realms issue 01 measurement and inventory (`.scratch/realms-and-concurrency/issues/01-*`) show what
that collides with:

- **Shapes are layouts, keyed by per-realm pointers, hung off one process-global root.** A
  `kiln_shape` holds no prototype: it is parent, interned keys and slots. But keys are interned per
  heap, and `kiln_c99_root_shape` is one static that every realm's transitions are written into. Two
  realms therefore grow one tree keyed by pointers that die with each realm's heap, so a freed key
  address reused by another realm can match a stale transition.
- **Inline caches hold objects.** A PIC entry remembers `holder`, `via` and `holder_value`, which
  are objects and values of the realm that taught it; the typed-callee cache remembers `_fn`,
  `_proto` and `_upvalues`. None of them is valid in another realm, and none checks which realm it
  is in.
- **The caches are the largest per-realm cost.** The emitted inline-cache arrays are 1.1 MB (hello
  world) to 3.1 MB (a small HTTP server) of zero-filled static storage, 188 KB and 368 KB of it
  touched. A per-realm copy of them alone is ten times the ADR 0070 target of tens of kilobytes per
  realm.
- **Anything shared between realms is written by several threads** once realms run in parallel,
  and a cache entry is several words wide.

Three responses are possible. Give every realm its own copy of every cache: correct, but it costs
the megabytes above per realm and every new realm starts cold. Tag every entry with the realm that
wrote it: correct, but realms running the same program on the same sites evict each other on every
call. Or make the caches mean the same thing in every realm, which is what this ADR decides.

## Decision

**A cache remembers layouts, paths and code, never objects. Keys and shapes are shared by every realm
in the process. Shared structures are published atomically and reclaimed at turn boundaries.**

### Keys

A key that is a constant of the program (every name in the compiled units' constant tables, and
every key the built-ins define) is interned once per process into a shared, immutable key table,
built before the first realm starts. `kiln_c99_intern_static` already pins these; it now resolves
into the shared table. A key made at run time from data (a computed name, a JSON field) that is not
already in the shared table is interned in the realm's own table, as today, and dies with the realm
or with the collector.

A key's identity is its pointer in both tables. The shared table is consulted first, so one
spelling is never two keys.

### Shapes

The shape tree is shared. Its root is one process-wide shape, and a transition by a shared key
from a shared shape creates a shared shape, inserted with an atomic compare-and-swap; shared shapes
are never freed while the process runs. A transition by a realm-local key, or from a realm-local
shape, creates a realm-local shape that belongs to the realm's heap and is collected with it.

Because a shared shape means the same layout in every realm, an object can move between realms
(ADR 0070, primitive C) or be read from the shared immutable region (primitive B) without its header
being rewritten.

### Caches

Every inline cache states its answer in realm-independent terms:

- **Own data properties:** receiver shape and slot, as today.
- **Prototype properties and methods:** receiver shape, the *depth* on the prototype chain, and the
  shape and slot at that depth. A hit follows the receiver's live prototype pointers that many
  steps, checks each shape, and reads the slot live. The `holder`, `via` and `holder_value` fields
  are removed. This is the same walk a hit does today, starting from the receiver instead of from a
  remembered object.
- **Built-in methods:** which built-in, not which object. It stays valid while the current realm has
  not modified that built-in, which each realm records for itself (see ADR 0070 primitive B: a
  modified built-in is the realm's own copy).
- **Typed callees:** the callee's *code* (its function prototype), not the function object. A hit
  checks the loaded closure's code pointer and reads its upvalues from the closure. `_fn`, `_proto`
  and `_upvalues` are replaced by that check.
- **Epochs** that validate a cache are per realm and are consulted only for what is per realm
  (whether this realm has modified a built-in or a prototype it relied on). A cache entry is never
  keyed by a realm's epoch value.

A cache stated this way can be shared by every realm in the process. The emitted cache arrays stay
single, process-wide statics, and a new realm starts with the caches every earlier realm warmed.

A cache that genuinely cannot be stated without an object is either restated (as above) or moved
into the realm's per-unit block, allocated on first use. The inventory in realms issue 01 lists
them; the goal is none.

### Publishing and reclaiming shared state

Shared keys, shapes and caches are written by whichever thread's realm learns something first.

- A cache whose answer fits one machine word is written and read with single atomic operations.
- A wider answer is an immutable record: a writer builds a new record and publishes it with a
  compare-and-swap of the site's pointer; a reader loads the pointer once and reads the record
  through it. A reader never sees half an update.
- A replaced record is retired, not freed. Every turn boundary is a quiescent state for the thread
  that reaches it (no generated frame holds a pointer into shared cache state there, which ADR 0041
  already guarantees for the heap). A retired record is freed once every scheduler thread has passed
  a turn boundary since it was retired: quiescent-state-based reclamation, with the turn as its
  quantum.

The single-realm hot path stays what it is now plus, for wide entries, one dependent load.

### What stays per realm

The realm context: the heap, frame registers, the symbol registry, `node:ffi` state, diagnostics,
module globals, namespaces and template sites, the per-realm epochs, and anything else realms issue
01 classified as realm state. It is reached through one pointer per function entry, using the
generation-checked cache that `kiln_c99_gc_heap` already uses.

## Consequences

- The per-realm cost of caches goes from megabytes to nothing, and a new realm is warm from its first
  turn. That is what makes a realm per actor, connection or tenant practical (ADR 0070).
- The shape tree stops being a use-after-free hazard across realms, and objects keep their shapes
  when they move between realms or live in the shared region.
- Removing objects from cache entries also removes the entries' reason to be registered with a heap
  for clearing when a holder dies (`kiln_c99_gc_register_site`): a path cannot dangle.
- Hot-path changes need measuring. The acceptance bar for each step is `mise run bench` and the
  small-service benchmark within noise of before, on one realm.
- The turn boundary now bounds three things: scheduling (ADR 0070), collection (ADR 0041) and
  reclamation of shared state (this ADR).

## Rejected alternatives

**A copy of every cache per realm.** Correct and simple, and it costs about 0.2 to 3 MB per realm
and a cold start for each. It survives only as the fallback for a cache that cannot be restated.

**Shared caches tagged with the realm that wrote them.** Correct, and realms running the same
program would evict each other at every shared site.

**A process-wide key table for every key, including run-time keys.** One table is simpler, but keys
made from data are unbounded and would need concurrent collection of the table. Constant keys are
bounded by the program and cover the sites that matter; run-time keys stay with the realm.

**Locks around cache updates.** Simple to write, and it puts a lock on the learning path of every
site in every realm, where an atomic publish costs nothing on the hit path and little on the miss.
