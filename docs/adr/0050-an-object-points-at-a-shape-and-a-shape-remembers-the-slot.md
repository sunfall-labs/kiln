# ADR 0050: An object points at a shape, and a shape remembers the slot

## Status

Proposed. The successor ADR 0047 names in *What the next step is*, and the last large item on the
native object model. Nothing here has to undo ADR 0047: the intern table is what a transition tree
keys its edges by, and the per-site slot cache becomes a per-site *shape* cache.

## Context

Measured with callgrind on the mixed server workload -- 2.58 billion instructions, steady state,
no instrumentation bias:

| | share |
| --- | --- |
| `kiln_c99_own_index` | 4.23% |
| `kiln_c99_own_slot_ikey` | 3.57% |

Nearly eight percent, and both are the same shape of work. An object is a flat array of
`kiln_property`, so finding a property is a linear walk comparing interned key pointers, and
*adding* one walks the same array first to prove it is not already there. Building the request
object writes twelve properties, which is 0+1+2+...+11 = 66 pointer compares before a single
one of them is stored; the response and the URL add more.

ADR 0047 made each of those compares a pointer compare rather than a `memcmp`, which is why this
is 8% and not 20%. It cannot make the walk shorter.

## Decision

### A shape is the layout; an object is the values

A shape describes *where* each key lives: an ordered list of `(interned key, attributes)` and the
slot index for each. Objects sharing a layout share one shape. An object becomes a shape pointer
plus a flat `kiln_value` array indexed by slot.

Adding a property follows a transition edge from the current shape, keyed by
`(interned key, attributes)`, creating the edge the first time and reusing it forever after. The
second request through the HTTP path walks twelve edges that already exist and writes twelve slots,
with no search at any step.

Reading becomes: compare the object's shape against the one this site last saw, and on a hit take
the remembered slot with no lookup at all. That is the ADR 0047 site cache with a sounder key --
today it caches a slot index validated by the key, which cannot tell two objects with different
layouts apart.

### Indexed keys keep their own store

The language orders own properties with integer-like keys first, ascending, and everything else in
insertion order. A transition tree cannot express that: inserting `2` into `{1, "foo"}` has to
place it *before* `"foo"`, which is not an append and so is not an edge.

Engines answer this by not putting indexed properties in the shape at all, and this should too:
an elements store beside the shape, and `kiln_c99_key_index` -- which already decides indexness on
the way in -- routes to it. This is the part of the change that is not optional, and the reason
this ADR is larger than "add a shape pointer".

### Deletion and unusual attribute work fall out of shapes

`delete`, and `defineProperty` with attributes that no transition covers, drop the object into a
dictionary representation: today's array, kept for exactly these cases. Objects that get there
stay there. This is what every engine does, and it is why the shape path can stay simple.

## Consequences

- The hot path stops scanning. Construction of a fixed-layout object -- which is what a request,
  a response and a URL are -- becomes a walk down an existing edge chain and a slot write.
- `Object.keys`, `for...in`, spread and `JSON.stringify` read the order off the shape rather than
  the object, so all of them get shorter too.
- Shapes are shared and immutable, so they are realm-lifetime and must be traced, or pinned and
  bounded. An unbounded transition tree is a leak for a program that builds many one-off layouts;
  a cap that falls back to dictionary mode is the usual answer and should be measured, not assumed.
- `KILN_KEYS_CHECK=1` gains a sibling: a mode that walks both representations and compares, which
  is what caught the interning invariant when ADR 0047 landed.
- This wants to land before any nursery work. A nursery changes what allocation looks like, and
  shapes change what is allocated; doing them in the other order means rebasing the harder one.

## Alternatives rejected

**A hash map per object.** Turns the 8% into a hash and a probe per access rather than removing
the work, costs memory on the small objects that are nearly all of them, and answers nothing about
property order.

**Sorting the property array and searching it.** Ordering is insertion order, not key order, so
the sort would have to be a second index -- the same bookkeeping as a shape without the sharing,
and without making construction cheaper.

**Leaving it and taking the nursery first.** Tempting, because allocation looked larger. On the
evidence it is not clearly larger: `gc_alloc` already serves small objects from slabs, and the
libc allocator time attributed to the request path is mostly `realloc` growth of buffers and
property arrays -- which is to say, partly this.
