# Realm bootstrap representation and optional startup image

Status: architecture review, 2026-09-28. The copied-image first cut below is deferred. The
shared-default representation is the primary implementation path for issue 08.

## Delivery checkpoint, 2026-09-29

Promoted nursery chunks now release complete pages crossed by no surviving allocation. In the
public held-actor workload this lowered 1,000-actor RSS by about 215 MiB, from roughly 823 to
608 MiB on this Linux host, without changing object addresses. It is a useful allocator step,
but still leaves an added-actor slope around 600 KiB: the shared-default blueprint remains the
retained-memory design. Page reclamation does not remove the repeated TypeScript bootstrap's
startup cost.

An opt-in side-capacity inventory of three later public actors found 140.6 KiB of GC payload
and 44.9 KiB of counted object, array and closure side buffers at the second turn, including
35.4 KiB of ordinary property arrays. The same binary's 100-to-1,000 held-RSS slope was about
598 KiB per actor. Those figures bound a property-array-only optimization and make page
residency an explicit part of blueprint acceptance: a compact layout matters only if direct
construction also avoids or releases the allocator pages holding the bootstrap graph. The
detailed counters and their exclusions are recorded in issue 08.

Keep the three public actor data paths distinct: structured-clone messages for ordinary calls,
`share()` for immutable graphs, and ownership transfer for buffers or explicitly prepared
graphs. `prepareTransfer` snapshots into a private arena, so the sender cannot retain an edge
into its storage; the receiver adopts that arena after preflight. Direct `isolate(factory)`
continues to require a strict incoming-alias certificate and is an optimization for graphs
already built in exclusive storage. It must not gate ordinary actor use or silently fall back
to a clone when its proof fails.

The public release and acceptance gate comes before the density representation work. The
measured actor has about 816 KiB of held-RSS slope and a roughly 2–3 ms warm spawn-to-ready
time, while repeated TypeScript bootstrap accounts for 1.6–1.8 ms of that startup path. These
are explicit capacity limits, but the actor API, bounded scheduler, messages, sharing and
buffer transfer are useful independently. A generic copy-on-write object layer is too invasive
to put on the critical path: emitted property reads, reflection, raw host consumers and GC all
need a complete identity and mutation contract.

For the next startup experiment, use the typed, packed copied image below only after its root
and relocation inventory is complete. It may remove repeated bootstrap execution and one
retained nursery chunk without changing the ordinary object read path; it cannot by itself meet
the tens-of-kilobytes density target. The shared-default blueprint remains the density design,
to be built as one measured actor-enabled subtree with a validated fallback. Keep the image
only if interleaved public actor measurements show the end-to-end benefit, and keep the
blueprint only if it materially lowers the 100-to-1,000 held-actor RSS slope without regressing
ordinary property access.

## Architecture decision after the public actor measurements

The first production milestone is the existing scheduled actor API: module-addressed actors,
structured-clone messages, explicit immutable `share()` values, and `ArrayBuffer` transfer.
These semantics work without general object-graph moves. Keep the density limitation visible:
100 to 1,000 held public actors cost about 816 KiB each. Finish the public acceptance gate and
documentation without making arbitrary `isolate(factory)` graphs a dependency.

For density and startup, build one process-owned **bootstrap blueprint**, not an ordinary GC
heap that every realm copies. It contains immutable property descriptors, key order, code,
default primitive values and a validated construction plan; bootstrap objects and captured cells
are named by image-local IDs. Each realm allocates stable identity handles, a compact table of
its own mutable values and host bindings, and sparse overrides for writes, deletes and descriptor
or prototype changes. The blueprint contains no realm pointer. An unchanged read projects the
shared default and the realm's identity; a mutation changes only that realm. Once an entire
bootstrap variant is validated, later realms instantiate the plan directly instead of running
the TypeScript bootstrap again. Partial or unsupported variants keep the ordinary bootstrap.
The generated fast path for ordinary user objects remains direct. This is the representation
specified under “Retained-memory end state” below, promoted to the main path.

Implement it by first measuring the *actor-enabled* retained bootstrap graph and classifying
every root, allocation kind and raw consumer that can see an image identity. Extend the
property-view and GC contracts to cover those consumers before activating a wide image handle.
Publish only a complete, validated blueprint; an unsupported bootstrap variant uses the
ordinary realm path. Start with a cohesive module/prototype subtree whose memory contribution
is measurable, then include the Node/actor bootstrap graph. Check identity, reflection,
mutations, collection and cross-realm isolation under stress. Acceptance is the public held-actor
RSS slope and spawn latency, not a core-realm microbenchmark alone.

The first root audit found three authorities. The C realm context owns symbols, graph escrow,
native prototypes, generated unit values and shared-region closure overlays; GC and the graph
incoming proof now read those slots through one typed enumerator in `runtime/heap.c`. The provider
owns value slots, callable stores, continuations, queued work and timers, exposed through its
read-only root visitor. The generated launcher owns import/export/namespace cells, module states
and results, exposed through its own visitor. Promise jobs, key-table entries, hot keys and weak
lists have separate C ownership rules; the graph proof checks their stored pointers even when
they are not strong GC roots. A blueprint must classify the *reachable edges* behind these roots
and each native host binding before sharing any object. Root enumeration alone is not a copying
or sharing certificate.

The copied-image scheme below remains an optional startup experiment. It requires nearly the
same root and relocation audit but cannot reduce retained object copies. Build it only if the
shared-default path cannot meet the spawn target and an interleaved benchmark shows a useful
end-to-end saving. A prewarmed actor pool would retain idle realms and is not the default.

The later-actor heap probe gives this experiment a concrete bound: after a forced bootstrap
collection, about 94 KiB of live nursery payload occupies two 256 KiB promoted chunks. A
correct packed copy could fit that nursery payload in one chunk, so a typed copied-image pilot
has a plausible one-chunk density benefit as well as the measured 1.6–1.8 ms TypeScript
bootstrap phase to attack. Neither saving is established until a complete relocation audit and
same-workload measurement; the three slab ranges and non-GC state also remain. See issue 08's
2026-09-28 chunk inventory.

The 2026-09-29 actor turn-boundary sample includes the target module and constructor. Later
actors held about 140.5 KiB of GC payload across two formerly full 256 KiB chunks and a third
chunk with only a 15.2 KiB allocation span. This strengthens the case for a packed startup image
as a measured intermediate capacity step, while bounding its likely gain below the full
816 KiB/actor held-RSS slope. Measure the image and the shared-default subtree separately; the
image must still pass the typed relocation proof below before activation.

The closure breakdown narrows the shared-default slice: of 181 retained closures in a later
public actor, 131 have captured upvalues and only 28 are ordinary and uncaptured. Sharing all 28
would remove about 6.1 KiB of payload before allocator effects. A late-closure-only extension of
the existing sealed region is too small to be the density milestone; the first measured subtree
must include realm-owned object identities, captured cells and the pages that hold them.

## Checkpoint before extending the compiler or object layout

The public actor API, scheduler, messages, shared snapshots and prepared graph handoff are a
usable first milestone. Publish and verify that surface independently of the realm-density
target. An arbitrary `isolate(factory)` graph move remains a separate, strict ownership proof;
it is not required for ordinary actor calls or for `prepareTransfer`.

Do not add allocation-site IDs or a wide bootstrap-object representation as scaffolding alone.
First measure the retained Node-enabled actor by ownership class: live payload, side buffers,
GC chunk and slab residency, native/provider state and thread stacks. The existing diagnostic
found 82,303 bytes of additional live GC payload after TypeScript bootstrap in a later actor,
while the held public-actor RSS slope is about 816 KiB. Those numbers measure different things;
sharing descriptors alone has no demonstrated path to the RSS target. Record the baseline and
measure a candidate on the same 100-to-1,000-actor workload.

The candidate should be one end-to-end bootstrap subtree: immutable process-owned metadata,
stable realm-local identities, compact realm-local values and overrides, and an audited
read/write/GC view. Construct later realms directly into that representation, so temporary
dictionaries and their allocator pages are not retained. Choose the subtree from the ownership
inventory, include a Node/actor bootstrap path, and require a measurable RSS and startup change
before extending the compiler's once-body proof to more allocation sites. Preserve the ordinary
object layout and fast path for user objects. After the slice, address allocator page residency
separately if the live-payload saving does not appear in RSS.

## Deferred startup candidate: copy a pristine image

The public `spawnActor` benchmark measures 2.80 ms from spawn to ready for a warm actor.
Temporary phase probes put 1.6–1.8 ms inside the native realm's TypeScript bootstrap, while
native-module installation takes 0.16–0.19 ms and the actor's target import and constructor take
less than 0.2 ms. The actor already uses the bounded realm scheduler. A pool of prewarmed actors
would retain idle realms and make the result depend on pool occupancy.

Run the ordinary TypeScript bootstrap in the first realm, before user code can modify built-ins.
Capture the bootstrap's final state over its intrinsic roots and unit cells. Some roots, such as
the global object and `console`, exist before the TypeScript bootstrap; record their bootstrap
changes against named roots instead of copying their first realm's native host fields. Use a typed
relocation table: each field is either a shared immutable reference (key, shape, string, compiled
code or region closure), a pointer to another copied object or cell, or a binding to the new
realm's corresponding root. Any unclassified pointer or object kind makes the image ineligible
and leaves the ordinary bootstrap as the fallback. Publish an eligible image only after the
bootstrap succeeds and the collector has established complete roots.

For a new realm, create its ordinary native roots, reserve and root its destination blocks, copy
the image's object headers, property slots, descriptors and cells, and apply relocations to those
blocks and to the fresh native roots before exposing the global object. Realm-specific native
handles, `process`, console streams, worker data and clock state must be created afresh or rebound
explicitly; they cannot be inherited from the builder. Run the existing
post-bootstrap sealing/cache setup against the new realm's objects, without counting a bootstrap
write in the process-wide mutation epoch. The new realm has distinct object identities and owns
all mutable property storage. Shared region closures keep their existing realm-local stores and
module-cell lookup. The first realm and any unsupported image continue to execute the TypeScript
bootstrap normally.

This candidate deliberately copies mutable slots rather than adding copy-on-write indirection to
the property read path. The C emitter currently reads ordinary object shapes and slots directly;
introducing a shared mutable-looking object would require a new check on every fast property
read, or a complete audit of every slow-path dereference. Copying roughly the bootstrap's live
heap is cheap enough to test as a startup optimization and preserves the existing object model.
It does **not** claim the tens-of-kilobytes per-realm memory target.

## Evidence required if the copied-image candidate is built

1. Inventory every block kind and pointer reachable from the bootstrap roots in a Node-enabled
   actor binary. Prove that each copied pointer is relocated, each shared pointer targets
   process-lifetime immutable storage, and each external pointer is rebound or rejected.
2. Compare fresh and image realms under native GC stress and a repeated multi-realm run. Check
   property writes, deletion, descriptors, prototype changes, and module-cell reads, including
   a built-in modified in one realm while another starts.
3. Measure the public actor-start workload on the same native-release binary configuration and
   check a single-realm property-read benchmark for regression. Keep the image path only if its
   improvement survives interleaved rounds; the 1.6–1.8 ms bootstrap phase is an opportunity,
   not a promised end-to-end saving.

The image is indexed by the binary's bootstrap variant and feature switches. A different
bootstrap source, extension setting or Node surface cannot reuse an earlier image. Publication
and fallback follow the shared region's builder/sealed rules so simultaneous actor starts cannot
see a partial image.

## Retained-memory end state: shared defaults, compact realm state

The copy image is a startup step, not the final realm representation. The public capacity probe
retains about 816 KiB per actor from 100 to 1,000 live actors, while a minimal core realm retains
about 97 KiB. A minimal scheduled Node worker retains almost as much as a public actor. The Node
worker and actor module bootstrap graph must therefore participate in the shared representation;
sharing only intrinsic closures or tuning the core heap cannot meet the actor-density goal.

Use the image's typed block and relocation inventory as the proof boundary for a second, compact
form. Its process-owned part contains immutable code, descriptors, property order, default
primitive values and references encoded as image-local identity IDs. It contains no pointer into
a realm heap. Each realm owns a compact table of stable identities, realm-specific host bindings,
mutable cells and sparse overrides. A read of an unmodified built-in resolves its default through
the immutable descriptor and that realm's identity table. A write, delete, descriptor change or
prototype change records a realm-local override; the process image never changes. Identity and
`===` remain stable even after the first mutation, and a mutation in one realm is invisible to
another. Shared reads must not teach caches, intern keys or write a memoized result into sealed
storage. The ordinary object fast path must stay direct for user objects; benchmark the extra
dispatch on intrinsic reads separately.

The blueprint must also describe how to instantiate its realm-owned portion: identity IDs,
mutable cell initial values, intrinsic prototype links and explicit host-binding operations.
Capture and validate that plan only after the first realm completes bootstrap; no raw pointer,
callable closure with realm captures, or first-realm host handle may be published. Later realms
allocate their compact state and bind their own host authorities directly from the plan. Until
the complete variant is certified, execute the existing TypeScript bootstrap. Sharing final
descriptors while replaying all of bootstrap would reduce some retained bytes but leave most of
the measured 1.6–1.8 ms startup phase in place. This plan is limited to proven bootstrap sites;
ordinary user objects retain the normal allocation and property paths.

Do not publish this representation until every direct `kiln_object *` consumer that can receive
an image identity has been classified: emitted fast paths, own-property and prototype walks,
reflection, GC tracing, graph transfer, host APIs and cache publication. Unsupported bootstrap
variants must use the existing ordinary per-realm construction path. Once the compact form
passes stress and multi-realm probes, measure the
100-to-1,000 public-actor RSS slope and warm spawn latency again; the core Date-plus-timer slope
alone is not an acceptance result.
