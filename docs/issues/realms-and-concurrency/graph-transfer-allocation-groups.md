# Object-graph transfer: owned allocation groups

Status: implementation design, 2026-09-28. Refines ADR 0070 primitive C and issue 10.

## Decision

An actor graph's movable unit is an **owned allocation group**, not an individual object and
not only a nursery chunk. The group contains dedicated nursery chunks, standalone
large GC blocks, and side allocations uniquely owned by blocks in the group. Addresses do not
change. Moving the group changes which realm's collector and finalizers own it.

The current capsule moves a narrow subset whose arrays and object slots happen to be inside
their GC blocks. That is useful, but making every large array or dictionary inline is the wrong
general rule. The normal array allocator deliberately switches to a separate `malloc` buffer
above 64 values; the array finalizer already owns and frees that buffer. An isolated 128-element
array should move with that owned buffer, without changing the allocation policy of ordinary
arrays or paying a copy. A separate large block and a 100 MB ArrayBuffer may use other storage
classes; no nursery-size threshold may define what the transfer API can accept.

This does not change the shared immutable region. A moved graph remains mutable and has exactly
one realm owner. Frozen user graphs keep the separate shared-value contract in ADR 0073.

## Architecture review and delivery order

The 2026-09-29 public actor measurement changes the priority of the direct path. A roughly
1 MB graph took 17.54 ms for direct handoff and reply after 8.31 ms to build, versus 12.93 ms
for a prebuilt-graph clone call and 9.78 ms for preparation plus handoff. These construction
conditions differ, but the direct handoff alone is slower than cloning the prebuilt graph.
Keep the three user-visible choices: ordinary clone for one send, `share()` for repeated
immutable reads, and explicit transfer for exclusive mutable storage. The direct factory path
must remain a strict optional optimization until it wins an end-to-end workload and has the
same observable contract in both engines.

Pause further per-layout expansion. Arrays, dictionaries, shaped slots, numeric object
elements and generated string literals now have candidate ownership. Standalone large
GC blocks now join that same manifest and use the existing audited layout walk; this adds
allocation-class coverage rather than a new per-kind field schema. Next replace the
sender-heap-wide proof with a complete candidate-lifetime escape ledger and bounded safepoint root
audit, then measure certification and total call time across both candidate and sender-heap
sizes. Widen direct layouts only after its proof cost scales with the candidate and both engines
have matching observable behavior. This optimization does not gate the scheduled actor release.

The scalable direct design is a genuinely private allocation group built during the factory,
with a candidate-scoped escape record for writes into older objects, host handles and external
resources. A handoff can then inspect the group's manifest, its escape record and the bounded
root/stack inventory at the turn boundary. Do not substitute a process-wide write barrier or
assume that a closed outgoing graph proves there are no incoming aliases. The current
sender-heap scan remains the conservative implementation until every escape seam has been
audited and tested; no partial remembered set may replace it. Measure proof, receiver adoption
and reply separately before widening accepted storage layouts.

The escape record must remain active after the factory returns, while its candidate exists.
An exposed factory result can later move from one outside holder to another before the first
alias is cleared. Tracking only factory-time stores misses that live incoming edge. The public
direct-graph probe exercises this sequence and requires refusal, followed by successful retry
once the second holder releases the alias.

Filled nursery chunks now carry intrusive predecessor links and explicit list membership.
Moving nonadjacent chunks validates membership and unlinks each chunk without walking unrelated
full chunks. Prepared arenas relinquish their temporary indexing heap's membership before
adoption. Candidate groups now have a separate intrusive manifest: discovery visits only the
candidate's chunks. The outgoing region index is retained for incoming membership checks, and
its dictionary-key flag and manifest build the capsule without a second outgoing walk. A
non-allocating consumer accepts the manifest only after every incoming authority passes under
the same sender lease; refusal restores escrow and disposes the proof's indexes. Collection
removes dead members without rooting the group; detachment and adoption release membership.
The incoming-alias proof still scans the sender heap and must not be described as
candidate-scaled certification.

The allocation-group certificate below is a sound, strict path for a graph already built in
candidate chunks. It is not the general actor messaging path. Its incoming proof currently scans
sender roots, machine words and the sender heap, so adding each new side-storage layout broadens
its accepted subset but does not remove its O(sender heap) certification cost or conservative
false refusals. Keep that path explicit and fail closed; do not turn a failed ownership move into
an implicit clone under the existing `transfer(handle)` contract.

For a broadly usable mutable graph move, prepare a **private transfer arena** containing a
snapshot of the supported graph. The sender receives only an opaque handle to that arena; any
aliases to the input still point to the original objects. The arena has no incoming sender edge
by construction. It has an allocation manifest for its GC blocks and uniquely owned side
buffers, and its outgoing edges may target only arena members or approved process-wide immutable
values. Preflight the receiver and publish the root under the existing atomic handoff protocol.
Preparation copies and validates the graph once; handoff then moves its storage without another
payload copy. This must be an explicitly documented preparation mode or API, not a silent fallback
for a requested zero-copy ownership move. A later direct-build optimization can avoid the
preparation copy only when it establishes the same no-alias invariant.

The proposed public spelling is `prepareTransfer(value): Isolated<T>`. It leaves `value` and its
aliases usable, snapshots the supported ordinary data graph into a private arena, and returns
only an opaque handle to that snapshot. The existing `transfer(handle)` ticket then consumes the
arena at message acceptance. `isolate(factory)` remains the distinct direct-build path: its
successful move copies no graph payload, and a failed certificate raises `DataCloneError` without
cloning. Both engines must expose the same contract and errors; the native backend can move arena
storage while the interpreter implements the equivalent ownership boundary. The accepted graph
must be stated explicitly, starting with structured-clone-compatible ordinary objects and arrays
whose keys, strings, shapes, side buffers and prototype semantics can be reproduced without a
pointer back into the sender realm. Unsupported hosts, accessors and weak ownership refuse.

Use one semantic graph walk and identity table for preparing a native arena and cloning ordinary
actor messages where their accepted types coincide. Make destination allocation a policy of that
walk: ordinary message storage or arena-owned blocks and buffers. This prevents two native
encoders from drifting on cycles, aliases, property order or errors. The arena allocator must
record every owned block and side allocation as it builds; sealing checks its manifest and
outgoing edges, then publishes one escrow root. Handoff must not rescan the sender heap because
the sender never received a pointer into the arena. A one-hop preparation still pays a graph
copy and must be benchmarked against ordinary structured clone, not advertised as a free move.

The existing native plain-data clone walk now has an arena allocation policy for supported shaped
objects, strings and arrays. It builds into separately owned nursery-format chunks and records
array side buffers in a rollback manifest; the sender heap does not index those chunks while the
copy runs. A failed walk frees the chunks and buffers without exposing a partial graph. The
runtime now certifies that outgoing closure through a temporary index of only the arena's chunks,
then discards the index. The arena owns its chunks and side buffers until an explicit discard or
receiver adoption; a failed walk or certificate frees them. Adoption preflights the receiver,
attaches the chunks and publishes its root without ever indexing the graph in the sender heap or
scanning the sender heap for incoming aliases. A focused fixture exercises rejection, sender
collection before adoption, adoption and discarding a multi-chunk graph. This establishes a
reusable storage and handoff operation, not yet the public private-arena operation: it still needs
the claimed actor-turn transaction. Its internal opaque arena is now owned by a weak native
resource entry keyed by a local JS handle: collection of an abandoned handle or realm teardown
discards the arena. The pair-claimed actor transaction now stages this certificate, preflights
the receiver, adopts the arena and consumes the weak handle only after inbox publication. A
pre-adoption cancellation leaves the handle and its arena usable. The public `prepareTransfer`
operation and matching interpreter path have since been connected. Prepared dictionary keys now
move with their arena records and use the receiver's by-spelling dictionary path. Direct candidate
dictionaries now carry provenance-backed property buffers and also enable that receiver lookup
rule. Shaped slots now also carry candidate provenance through resize and finalization. Numeric
element side storage now carries the same provenance. Standalone large GC blocks now
carry cold owner entries, an indexed certificate and exact unlink/adoption under this same
transaction. Ordinary block headers retain their size. Metadata is not a collector root;
collection and explicit release remove it, and receiver adoption resets sender mark epochs.

Dynamic property names use arena-owned key bytes. A prepared graph cannot retain the sender's
realm-local interned key or insert every user name into the process-wide shared table forever.
The private clone copies each key into the same movable arena as its dictionary record, and the
outgoing proof follows that key edge. The capsule records this layout before the atomic handoff.
The receiver then uses pointer-first, spelling-second dictionary lookup and does not infer
absence from an intern-table miss while such a dictionary may exist. The per-realm flag is
conservative until teardown; ordinary shaped reads and realms without moved dictionaries keep
the interning fast path. This requires no allocation or GC root between receiver preflight and
publication. A future per-dictionary hash index could recover fast misses without changing key
ownership or the transaction.

For direct candidate dictionaries, the property record buffer needs the same allocation-time
provenance as an array element buffer. `kiln_c99_object_reserve_properties` is the ordinary
growth authority; its first allocation from an in-block dictionary and every later resize must
preserve the candidate owner, byte capacity and generation. The finalizer and every conversion
that replaces the store must free its allocation header, not the interior property pointer.
Sealed-layout conversion allocates its own record buffer and remains an explicit refusal. An
outgoing walk validates each record, key, descriptor and value, and adds the whole allocation
to the side-span certificate; the incoming
machine and outside-object scans must reject aliases to any byte of it. A process-shared key is
an allowed immutable outgoing edge. A sender-realm interned key is not: preparing a graph copies
its spelling into the movable arena, while a direct candidate refuses unless its key bytes and
all intern-table incoming edges have a valid ownership rule. Shaped slots and numeric-element
buffers now have allocation-time records, including capacity and generation. This keeps the
normal object path free of candidate bookkeeping and makes every accepted side allocation
auditable at its actual allocator, resize and finalizer sites.

Keep the user-facing release sequence centered on scheduled actors, ordinary messages, shared
immutable values and `ArrayBuffer` transfer, which are already usable. Finish the scheduler and
bootstrap startup work, then benchmark the private-arena preparation path against structured
clone and the strict candidate path before expanding the latter to every C storage kind. Large
buffers retain their existing transfer-list route. The remaining issue-10 contract still needs
implementation and verification; this ordering does not mark it complete.

## Certificate

At the sender's turn safepoint, build a temporary certificate from the actual objects and
allocations. Its entries are:

| entry | evidence required |
| --- | --- |
| GC allocation | exact kind, size, candidate provenance, finalizer bit, and owning chunk or large-block record |
| Side allocation | base, capacity in bytes, one owner block and field, and destructor; no other owner |
| Value edge | exact tagged target, or a documented process-lifetime immutable reference |
| Raw edge | exact target GC block or owned side allocation, with offset and extent checked |
| External resource | an explicit transfer operation and destination ownership rule, otherwise refuse |

The per-kind field schema enumerates **all** GC edges, raw pointers, side owners and finalizer
actions. Unknown kinds or fields refuse. The collector may use the same structural field
enumeration, but transfer has stricter rules for weak references, side ownership and
process-shared pointers; a collector mark result alone is not a transfer certificate.

For the first side-storage kind, a candidate-scoped allocation record follows each array
element-buffer `malloc`, `realloc` and `free`. It names the owner array, current base, byte
capacity and allocation generation. The array is eligible only when its pointer and capacity
match that live record, its count does not exceed capacity, and no second owner is found. The
certificate walks every live element. A buffer created before the candidate scope, or a stale
or replaced buffer inferred only from an arbitrary pointer, refuses. Subsequent
kinds include dictionary properties, shaped slots, object numeric elements and large data
buffers after their own ownership audits.

The proof covers **every** block and side allocation in the candidate group, including factory
temporaries. Unreachable temporary blocks may move as garbage only when their complete edges,
side owners and finalizers are accounted for. Edges from the group may name another group member
or process-shared immutable data; a realm-local outgoing edge refuses. The incoming proof
rejects pointers to any group block **or side span** from sender roots, generated and provider
roots, machine words, GC blocks outside the group, side allocations outside the group, host
handles and weak/finalizer state. It excludes only the consuming escrow slot. The source-level
name `isolate` never supplies proof.

## Transaction

1. Build and validate the manifest while the sender is exclusively leased. No candidate
   allocation, `realloc`, collection or mutator may run after this snapshot.
2. Reserve all receiver indexes, chunk and large-block capacity, mailbox space, and side-byte
   accounting before detaching anything. A failure leaves the sender's ticket and graph live.
3. With both realms claimed, unlink the full group from the sender, attach it to the receiver,
   publish the receiver inbox root, then consume the sender escrow. No fallible operation may
   occur after the first unlink.
4. The side allocation's address remains the same. The receiver's GC trace visits its values;
   its finalizer eventually frees it exactly once. Sender teardown cannot see or free it.

The existing chunk capsule is the first implementation of this protocol. Extending it should
reuse its pair claim, preflight, detach/adopt and rollback boundaries. Ownership metadata is
derived or registered at allocation, never guessed from a process address alone.

## Performance and acceptance

Zero-copy describes payload movement; it does not mean proof is constant-time. The current
outgoing walk and incoming sender-heap scan are proportional to graph and sender-heap size.
Measure both separately against structured clone. If the incoming scan dominates, add a
candidate-scoped remembered set or write barrier for cross-group stores while retaining the
root and host-handle audit. Do not place that barrier on ordinary single-realm writes.

Implement in this order:

1. Audit array element-buffer allocation, growth, finalization and every raw-pointer escape.
   Add a side-span entry to the certificate and incoming machine/outside scans. Transfer a
   128-element and a much larger side-backed array through the public actor API; refuse an
   injected second owner or retained sender alias.
2. Admit dictionary and shaped-object side storage with the same field schema. Check keys,
   descriptor attributes, insertion order, prototypes, mutation after adoption and GC stress.
3. Add standalone large GC blocks and other audited storage kinds. Keep ArrayBuffer's existing
   constant-size transfer-list path independent of object-graph certification.
4. Benchmark certification, handoff and clone separately across graph sizes and sender-heap
   sizes. Run native parity, multi-realm and collector-stress gates.

The broad acceptance target is arbitrary isolated ordinary JS object and array graphs whose
storage and edges are fully described by the certificate. Unknown host resources, realm-local
prototypes, weak ownership and retained aliases must continue to refuse explicitly until their
ownership rules are defined.
