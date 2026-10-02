# Reference-store coverage inventory

Status: candidate-scaled admission implemented, focused verification in progress, 2026-09-29.
The full outside-heap scan remains the strict diagnostic oracle. Public scaling and final
collector/gate acceptance are still required before landing the production switch.

## Evidence and scope

The Clang AST of the current linked callable-slot fixture found 1,942 direct assignments whose
left-hand type is a tagged value or a pointer field. This includes stack locals, caller-owned
outputs and native metadata; it is not a count of missing heap barriers. The raw compiler input,
AST and assignment list stay in `kiln-wt-property-stores/target/`. Their hashes and exclusions are
in `performance-evidence/graph-ledger-reference-inventory-2026-09-29.json`. Macro source ranges
without direct offsets are excluded; the lexical field and bulk guards remain a separate check.

## Authorities to retain

| storage | current authority | remaining coverage check |
| --- | --- | --- |
| Ordinary GC values and borrowed pointers | `heap_store_val`, `heap_store_value`, `heap_store_pointer`; current owner layout is inspected at certification | All indirect output slots and aliases, including aliases copied to another variable |
| Complete property descriptors | `heap_store_property` and `heap_copy_properties`; commit preserves the canonical key | Projection builders may write local descriptors; prove every heap publication commits through the authority |
| Same-owner slot relocation | `heap_move_vals`, `heap_relocate_vals`; weak witness identifies the owner, not its slot address | Guard proves the immediate source is the same owner's slots/elements; generic aliases require separate classification |
| In-block storage pointers | `heap_store_interior_pointer` and audited construction offsets | Self-owned string data and private-arena initializers also contain direct stores; prove ownership from allocator output rather than treating them as borrowed edges |
| Realm, generated-unit, provider and machine roots | Independent root visitors and turn-boundary certificate | Root slots are not GC-owner witnesses; retain every authority during ledger promotion |
| Host blocks | Shared `KILN_C99_HOST_LAYOUTS` inventory for GC, incoming scan and publication | Raw host links need either store recording or a complete typed-target exclusion; a field list alone does not prove all writes covered |
| Native side metadata | Key/shape, jobs, weak-resource, escrow and generated launcher inventories | Check stored pointers independently of GC strength; audit opaque payloads and context graphs before declaring them clear |

The callable-prototype alias defect was found here: `put_mode_key` returned a prototype storage
slot and assigned through `*slot`. The field-name guard could not see the write. The fix records
the actual closure owner, and a new immediate-alias guard covers prototype/store/exotic accessors.
Both the executable fixture and public actor refusal/clear/retry probe pass in strict oracle mode.
The whole-descriptor commit also now uses the common descriptor authority.

Generated compact property-cache hits were a separate uncovered source: monomorphic and
polymorphic writes now pass their object owner to `heap_store_val`. A warmed public two-shape
probe produced two strict-oracle disagreements with the previous CLI and none with the fix.
Generated template arrays, their properties holder and namespace tagged-value fields now use
owner-carrying stores too. A generated-source guard and an executable namespace candidate-export
fixture cover those paths. Canonical key, buffer relocation and other pointer metadata remain
separate obligations; a runtime-only source guard cannot prove emitter coverage.

## Next audit boundary

Classify producer, mutator, bulk copy and output-parameter paths for every incoming GC kind,
using the allocator's storage ownership and field target types. A pointer to a host resource or
self-owned tail is a different proof from a borrowed JavaScript graph pointer. Do not omit a
raw pointer merely because the collector ignores it.

`KILN_GC_COMMONJS_REQUIRE` now carries its launcher's existing visitor and stable anchor.
The kind walk checks direct pointers and validates the exact live registration with the active
heap. The independent root pass inspects module values once. Legacy opaque contexts remain
unknown. Ordinary and shared launchers use the same read-only inventory for collection and
transfer: marking is an adapter over the visitor. The public CommonJS probe checks move,
retained-export refusal and clear/retry under strict audit and concurrent realms. Focused evidence
is in `performance-evidence/graph-ledger-commonjs-verification-2026-09-29.json`.

After the store and metadata inventory is complete, retain strict comparison with the full walk
and measure fixed candidates against independently growing unrelated sender heaps. Only then
promote the ledger to production admission. The local-shape walk also remains sender-wide and
needs a complete canonical-key/publication authority or candidate-indexed metadata witnesses.


## Candidate-scaled authority closure

The implementation and remaining acceptance evidence are recorded in
[transfer-scaling-authorities.md](transfer-scaling-authorities.md). Callable aliases now include
simple copied and indexed slots. A transitive output-parameter audit reviewed helper-mediated
member writes; constructors either receive the owner or immediately publish their output pointer
through its owner. Native string tails own their allocation, and sealed Rust arenas are allocated
independently of realm heaps. Shared-facade links include the previously omitted hash chain.

Canonical key copies explicitly decline candidate capture at every size. Native shape extension
validates key identity, so the shape/key authority can replace sender-wide inspection of those
indexes. CommonJS context registrations have an independent weak inventory because both their
creation and their revocation can occur without an incoming candidate store. Unknown contexts
and incomplete inventories refuse. The root visitor and machine-root authorities remain intact.
