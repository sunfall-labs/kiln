# ADR 0047: Property keys are interned, and a named-access site remembers its slot

## Status

Accepted, and implemented for the C backend. The interpreter is not touched by this decision.

## Context

The native object model (`kiln_object` in the emitted prelude) is a list of `kiln_property`
records scanned linearly. Its own comment records why it has no shape: a shape identity has to be
globally stable and never recycled to be sound (ADR 0021), and inventing one before there was
anything to guard is how that goes wrong. A list has no identity to get wrong, and it is uniformly
slow rather than fast-until-megamorphic, which was the honest starting point.

Two things made the scan slower than a list has to be.

A key was compared by content. `kiln_c99_same_key` tried the pointer first, then the length, then
the first byte, then `memcmp`, and the pointer short-cut hit only inside one translation unit: a
literal name is one static array *per unit*, so `req.url` in two modules is two pointers, and a
computed key — `obj[name]`, a `JSON.parse` key, an `Object.keys` result — is a heap string that
never matches a literal by address. Every same-length mismatch in every scan paid a byte compare.

Nothing remembered anything between accesses. `this.count` in a loop scanned from slot zero every
iteration, and a class method reached through the prototype scanned the instance and then the
prototype on every call. The 2026-09-01/02 perf sessions attribute about 14% of a saturated request
to the object model, most of it in this lookup.

## Decision

### Interning is total for stored keys, and the table is the heap's

The realm's heap (`kiln_gc_heap`, the one struct every translation unit shares and the runtime
archive keeps, ADR 0041) owns an open-addressed table from key bytes to one canonical pointer.
**Every** write of a `kiln_property.key` goes through it. There are two such writes in the prelude,
`kiln_c99_own_slot` and the namespace wrapper, plus a symbol registering its reserved `@@symbol:`
key when it is created. That is the invariant everything else rests on, and it is checked rather
than assumed: `KILN_KEYS_CHECK=1` re-runs every pointer scan by bytes and aborts naming the key if
the two disagree, and the native corpus is run that way under `KILN_GC_STRESS=1`.

Three entry points, and the difference between them is the design:

- `kiln_c99_intern(bytes, len)` inserts. A `KILN_GC_STRING` block of exactly `len` bytes is
  adopted as the canonical bytes; anything else is copied into a block of its own. The
  exact-size rule is what keeps canonical pointers distinct as pointers.
- `kiln_c99_intern_static(bytes, len)` inserts for bytes that outlive the heap and **pins** the
  entry. It returns NULL only when the table cannot grow, and a unit that gets NULL for one of
  its keys fails its entry point with out-of-memory rather than run a site with a pointer that is
  not canonical.
- `kiln_c99_interned(bytes, len)` looks up without inserting. Because storage is total, a miss
  means no object anywhere has the key, and the read paths skip the scan on a miss.

The converted paths (`get_own_kind`'s object walk, `put_mode`, `own_slot`, `has_property`,
`delete`, `own_private`, `write_would_accept`) canonicalize the query once and compare pointers
through `kiln_c99_own_index`. A caller that already holds the canonical pointer — a compiled site
— passes it through the `_ikey` forms of `get`, `put_mode` and `own_slot` and hashes nothing. The
memcmp fallback is gone from the converted paths. The unconverted cold paths keep
`kiln_c99_same_key`, which is still correct against canonical storage — bytes-equal is still
key-equal — and whose pointer short-cut now hits more often, not less.

### The table is weak, and the collector knows it

An unpinned entry's bytes are a string block that the properties holding the key keep marked; the
table adds no edge. After the runtime roots are marked, the collector marks the blocks of pinned
entries (a unit can register a spelling a computed key already claimed, and the table has then
undertaken to keep that block alive); after the drain and before the sweep, it drops every
unpinned entry whose block is unmarked, in place, so the purge needs no memory at the one moment
the collector must not fail for lack of it. `kiln_c99_gc_release` of a string block forgets its
entry. The table is freed with the heap at teardown. The fallback heap a realm-less prelude test
uses never collects and its table only grows, as that heap already does.

### A unit interns its keys once per realm

The emitter collects every constant a `GetProperty`, `SetProperty`, `SetPropertyStrict`,
`CallMethod`, `DeleteProperty`, `DeletePropertyStrict` or `LoadSuperProperty` names and emits
`static const char *{symbol}_key_{n}` for each. `{symbol}_body` fills them with
`kiln_c99_intern_static` on its first entry; the unit's `KILN_GC_PHASE_RESET`, which the runtime
calls at realm destroy for every registered unit, clears the flag so a second realm in one
process registers against its own table. Those sites pass the canonical pointer. A site's pointer
is only ever an optimization: every entry point looks the query up by bytes, so a stale or
uncanonical site pointer costs a cache miss and nothing else.

### A site remembers a slot, validated by the key rather than by a shape

Each `GetProperty`, `SetProperty`, `SetPropertyStrict` and `CallMethod` carries a block-scope
`static kiln_c99_site` holding `own` (a slot index on the receiver) and `proto` (a slot index on
the receiver's immediate prototype).

A read hits when `own < count && properties[own].key == key` and the slot is a data property.
That is exact without any object identity: a key is on an object at most once and the pointer is
canonical across the realm, so the slot holding this key *is* the property the slow path's first
scan would find — on any object whatsoever. Objects built by the same code put their keys at the
same indices, which is what makes one guess right for every instance of a class that does not
exist. The prototype hit is taken only after the receiver's own list has been scanned and missed
by pointer, and only when the immediate prototype is an ordinary object, which is precisely the
slow path's second step. A write hits when the own slot is a writable data property on an object
that is neither frozen nor a module namespace, the case the slow path answers by assigning in
place. Accessors are never answered from a site: reading or writing one is a call.

A site never learns a `@@` key (machine-private, hidden node slot, symbol — each with hiding rules
of its own) or `__proto__` (a relinking rule), so the slow path's rules stay the only rules for
those. A site whose name the slow path keeps answering from somewhere it cannot cache (a builtin
method table, a deeper prototype) stops trying to learn after eight such answers, so an
uncacheable name costs one compare and nothing more.

### What the next step is

This is deliberately the first step, not the last. Without shapes, the prototype case still
scans the receiver to prove the own miss, a megamorphic site thrashes between two guesses rather
than being detected, and a `for...in` or `Object.keys` still walks the list. The next step is a
transition tree — a shape per insertion sequence, an object pointing at its shape — at which point
`own` becomes "slot under this shape" guarded by one pointer compare, the own miss becomes a shape
compare, and a site that has seen several shapes can be given a small per-site table instead of
a guess. Nothing here has to be undone for it: the shape adds a guard, the slot index is the same
field, and the intern table is what a transition tree keys its edges by.

## Alternatives rejected

**Interning at the compiler only.** A per-program key table the emitted C indexes would make two
units agree, but not a unit and a `JSON.parse` result, and the property-heavy workloads this is
for (a request's headers, a parsed body, `Object.keys` over either) are exactly the runtime-created
case. The runtime table subsumes the compile-time one: a unit's constants are just pinned entries.

**Keeping the memcmp fallback everywhere.** It costs nothing on a hit and something on every
mismatch, and the mismatches are the scan. Worse, it hides an interning bug forever. The check
mode exists so the fallback can go.

**Caching by object identity.** A `(last object, index)` cache hits only when the same object
comes back, which a request handler never sees. Validating by the key at the slot is what lets one
guess serve every object of a layout, and it is the reason the cache is sound without a shape.

**Strong interning.** A table that roots everything it holds leaks a program that uses objects as
dictionaries with unbounded keys. Weak entries cost one block lookup per entry per collection.

## Consequences

- A key comparison is a pointer comparison on every converted path, and a read of a spelling the
  realm has never stored does no scan at all.
- A monomorphic named access on a plain object is a bounds check and a pointer compare. Measured
  on `.scratch/interned-keys/bench-props.mjs` (native-release, 2M iterations): an own read fell
  from 32.5 ms to 14.8, a prototype read from 34 to 21, a prototype method call from 107 to 75,
  an own write from 114 to 102.
- **A lookup that arrives without a canonical pointer now hashes the key once**, and for a small
  object that costs more than the byte scan it replaced: a computed read `o[k]` went from 85 ms
  to 104 in the same benchmark, `"y" in o` from 48 to 63, and building a two-member object
  literal and reading one member back from 200 to 238. The `node:http` mixed workload
  (`.scratch/perf/app-props.mjs`, four client processes) is where that shows: the prelude builds
  every request, response and URL object with prelude literal keys, each of which is hashed on
  the way in, and the workload measured about 5% *below* the baseline (interleaved runs, base
  46.4k / 44.3k / 43.9k / 44.1k rps against 41.2k / 41.5k / 42.2k / 42.0k). The next step for
  that workload is the one the site already takes for compiled code: a pre-interned static per
  prelude literal key, validated against the realm rather than re-hashed. It is not in this
  change because it touches several hundred prelude call sites and wants its own measurement.
- `kiln_gc_heap` grows by the table. The runtime reads only the hooks word, so the Rust side is
  unchanged and no ABI version moves. `kiln_c99_keys_check_mode` is a new prelude static, listed
  as test-fixture state in the ADR 0036 manifest.
- Every emitted unit gains its key table, a reset line in its roots function, a prologue line in
  its body, and a static per site. The snapshot diff under `crates/kiln-backend-c/tests/snapshots`
  is exactly those lines and the seven instruction forms.
- `KILN_KEYS_CHECK=1` joins `KILN_GC_STRESS=1` as an acceptance mode for the native corpus.
- The interpreter's object model is unchanged, and the two engines still print the same bytes for
  the whole native corpus.
