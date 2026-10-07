# Bootstrap object layout: issue 08 design and activation audit

Status: design, not implemented. ADRs 0070 and 0071 set the ownership and publication rules.

## Measured bound

On `7b35fd23`, a temporary diagnostic forced a collection at
`kiln_c99_builtin_bootstrap_done` and walked the live GC blocks of a core
`native-release` `hold` realm. The diagnostic was removed. It found 96 live
`kiln_object`s: 76 shaped, 20 list mode. All list-mode property arrays together
have 17,472 bytes of capacity. The largest are:

| Final count / capacity | Property bytes | Identity |
| ---: | ---: | --- |
| 95 / 128 | 6,144 | global object (inferred from its construction path) |
| 47 / 64 | 3,072 | `Date.prototype` callable store |
| 36 / 64 | 3,072 | `%TypedArray%.prototype` callable store |
| 24 / 32 | 1,536 | another bootstrap list store |
| 15 / 16 | 768 | another bootstrap list store |
| other list stores | 2,880 | smaller stores |

The previous retained-heap profile measured the 96 object blocks at 23,040
bytes of rounded GC slots and about 25 realm-local shape allocations at 10,429
bytes. A perfect 8-byte-per-value replacement of **all** 17,472 property bytes
would save at most about 15 KiB of allocation payload; Date and typed-array
stores alone are bounded at about 5.5 KiB. This is an optimistic bound before
layout headers, allocator rounding, fallback dictionaries and mutations. A
standalone wide-descriptor retrofit cannot take the current roughly 136 KiB
added-realm slope to tens of KiB. Constructing into the new layout, without
first allocating and growing dictionaries, is necessary for a larger RSS cut.
Even that leaves the ordinary object blocks and realm-local shapes unless the
object representation changes too. The forced-GC nursery profile found live
blocks on 14 of 16 touched pages, so decommitting dead nursery pages alone has
an approximately 8 KiB ceiling.

## Representation to implement

1. **Stable site identity.** Extend the compiler's `module_cells` once-body
   proof to assign a site number to an object allocation that happens exactly
   once per realm and on no cycle. Exclude direct `eval`, unknown host-created
   objects, and sites whose allocation count depends on a program branch. A
   process table is keyed by `(bootstrap unit, site)` or by an established
   `(KILN_VALUE_BUILTIN|METHOD, code)` callable identity. Neither a first
   realm's `kiln_object *` nor a realm-local key may enter the table.
2. **Sealed process descriptor.** Each eligible site publishes one ordered
   vector of shared canonical keys, property attributes, accessor/data kind,
   method code where constant, and a prototype site or builtin identity. It
   holds no `kiln_value`, `kiln_val`, realm object pointer or GC root. The first
   realm builds it privately, validates the complete sequence, then publishes
   it with release/acquire under the process lock. Other realms validate
   layout identity; a mismatch stays on the ordinary object path. Publishing a
   *prefix* is forbidden because a reader could mistake an unfinished object
   for the final global/prototype surface.
3. **Realm identity and values.** Every realm has its own stable object handle
   for each activated site and its own dense value slots and mutable prototype
   link. A handle may be much smaller than today's 200-byte `kiln_object`, but
   it still needs a GC-recognized identity and teardown behavior: an object
   returned through the borrowed-result contract may outlive its realm. A
   realm block containing values is traced explicitly; a process descriptor
   never points into it. The handle's identity must remain stable through
   copy-on-write materialization, and a later realm must never see another
   realm's override.
4. **Reads and writes.** A single property view answers lookup, own-key order,
   descriptors, spread, JSON and prototype walks from shared descriptors plus
   realm values. The view is read-only and may run in the no-write parallel
   path. A value assignment updates the realm slot only when its attributes
   allow it. Adding, deleting, reconfiguring or overriding a recorded method
   materializes a dictionary for that one handle before mutation; subsequent
   reads see that dictionary. An IC guard needs layout identity *and* the
   handle's realm-local mutation token, and must never cache a realm value in
   process-wide state.
5. **Build directly.** After the first descriptor is sealed, later bootstrap
   executions write eligible sites' values straight to their pre-sized realm
   slots. They do not allocate and grow a `kiln_property` array or build a
   chain of prefix shapes first. The first realm can use the ordinary path;
   its one-time cost is process startup, not the added-realm slope. A site not
   proven eligible keeps the current path. Skip entire bootstrap subtrees only
   after checking global enumeration and descriptor observations; skipping
   `process.__kilnNativeModules` alone previously saved only 0.8 KiB.

## Required access audit before activating a wide handle

The current `Object.prototype` final shape works because it has four entries
and reuses the ordinary shaped-object paths. It is not a general handle. A raw
source search at this commit finds 370 `->properties` uses across 27 runtime C
files, 77 `kiln_c99_ensure_dictionary` references and 190
`kiln_c99_object_of` references. These are search counts, not a count of required
edits: each needs classification by read, write, GC or host escape. In
particular:

| Seam | Required proof/change |
| --- | --- |
| `read.c`, `boxed.c`, `methods.c` | Ordinary and callable reads use the descriptor/value view without materializing. `kiln_c99_callable_store` must not turn a Date or typed-array method read into a dictionary. |
| `keys.c`, `enumeration.c`, `spread.c`, `json.c`, `definitions.c` | Own-key order, enumerable and symbol visibility, descriptors, accessors, spread and stringify see the same values and attributes through the view. Existing ordered-property projections are the starting seam. |
| `write.c`, `properties.c`, `definitions.c` | Every mutation either writes a permitted realm value slot or materializes that realm's dictionary before obtaining a writable property pointer. Freeze, seal, deletion, prototype changes and method overrides invalidate ICs. |
| Generated C and `shapes.c` | A final layout larger than 32 never enters the shape's 32-bit masks, `keys[32]`, or prefix transitions. Fast paths decline or guard on the new layout plus mutation token; ordinary shapes keep their size. |
| `heap.c`, graph transfer and teardown | GC traces every realm slot and prototype edge; conservative candidate lookup validates a handle before dereferencing it. Transfer cannot move a handle that still names a realm overlay. Borrowed-result survivors retain or materialize all state they need after teardown. |
| Node/host boundary | `kiln_value` tags, `typeof`, equality, `instanceof`, `Object.getPrototypeOf` and host callbacks preserve per-realm object identity. Cross-realm messages keep their existing clone/transfer rules and never alias the sender's overlay. |

Activation gates: at least two realms with different mutations to the same
builtin; `Reflect.ownKeys` and descriptors before/after writes; a normal Date
method read that leaves the compact layout attached; GC stress at 1, 2 and 8
realms; no-write parallel reads; borrowed results across realm teardown; and
same-binary `KILN_REALMS=1/100` anonymous-RSS slope. Keep issue 08 open until
the slope reaches the tens-of-KiB target and the final `mise run native-stress`
passes.

## Global direct-construction activation blocker (2026-09-28)

The complete-descriptor writer removes the global bootstrap's caller-owned
`kiln_property *`, but it does not make the global a final store. The C
`kiln_c99_global_object` inserts constructors, methods, realm objects and
temporary `__kiln*` hooks. The compiled built-ins then capture and delete
many of those hooks (`web_events_streams.ts`, `typedarray.ts`, `url.ts`,
`arraybuffer.ts`, `extensions.ts`, `structured_clone.ts`, `atomics.ts` and
others). `enumeration.c`'s delete path shifts `object->properties` in place.
A sealed initial table with dictionary materialization on the first deletion
would therefore allocate the same per-realm property array during every
ordinary bootstrap. Sealing only the post-bootstrap table would avoid that
mutation, but later realms still have to execute the earlier bootstrap reads
and writes; it does not by itself give them a direct-construction path.

The required next refactor is an object-wide property projection, usable
before a dictionary exists. The existing `kiln_c99_property_walk` and
`kiln_c99_property_view` in `shapes.c` project *shaped* objects, but a wide
global has more than the 32 entries representable by `kiln_shape`'s masks and
arrays. Extend that interface to report a sealed descriptor plus realm-owned
value, a deleted-key tombstone, or a realm-only appended property in the same
own-key order. It must never materialize on a read, including the no-write
parallel path. In particular, migrate these current direct consumers before
activating a sealed global:

| Current consumer | Why the existing list assumption breaks |
| --- | --- |
| `globals.c` `global_binding` and `_key` | Every bare global read scans `object->properties` and returns its embedded value. A sealed global with no array would make bare names disappear. |
| `read.c` ordinary and prototype reads; `boxed.c` generic and IC reads | These use `own_index_key`, direct records, or shape masks. A sealed handle needs a read-only lookup and an IC guard tied to its layout and per-realm mutation token; a realm value must never enter process cache state. |
| `keys.c`, `enumeration.c`, `spread.c`, `json.c` and descriptor readers | Own keys, enumerable names, symbol visibility, getters, spread, stringify and descriptors must see the projected values and tombstones in the same order. The current view covers only selected key walks. |
| `properties.c`, `definitions.c`, `enumeration.c` mutation paths | Assignment, method definition, `defineProperty`, deletion, freeze and extension need a realm-local overlay or materialization. Bootstrap hook deletion specifically must remain in the overlay, or the proposed memory saving disappears immediately. |
| `heap.c` GC trace and graph incoming proof | `KILN_GC_OBJECT` currently traces `object->properties[i]`, shaped slots and prototype. It must trace every realm value, appended record, accessor half and prototype, while treating process descriptors and shared keys as non-realm edges. The graph proof must reject an unenumerated overlay. |

The representation must keep each global object's identity and mutable values
in its realm, and publish only canonical keys/attributes/method codes to the
process after a complete first-realm validation. A discriminator for a wide
view needs an audited ownership rule: making `properties` point at a tagged
descriptor is unsafe while existing free, realloc and direct indexing sites
remain; treating it as a 33-plus-entry `kiln_shape` invokes undefined
`1u << slot` shifts. The projection and mutation/GC contracts are therefore
prerequisites to activating any wide descriptor, rather than follow-up polish.

The prior retained-heap profile bounds *all* list property arrays at 17,472 B
per realm. Even an ideal shared-descriptor/eight-byte-value representation
of every one saves under about 15 KiB of live payload, and the global array
alone is 6,144 B. A global-only virtual representation would add substantial
semantic surface for a ceiling of a few KiB and leave the roughly 136 KiB
added-realm slope far above tens of KiB. No active global layout was retained
from this audit.
