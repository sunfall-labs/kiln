# Actor graph handoff transaction (issue 10)

`isolate(factory)` now stores the result in an engine-owned realm registry keyed weakly by the
handle; `transfer(handle)` stores the handle in a JavaScript `WeakMap`. An actor call handles its mailbox
`grant` in a generated JavaScript callback, prepares a clone, and calls `MessagePort.postMessage`
from that callback. Neither the callback nor `postMessage` is a sender turn safepoint. The
current graph-ticket `DataCloneError` is required.

## Engine-owned request and boundary

The narrow first implementation should accept only the C graph kinds for which the outgoing
walker proves closure: inline and sliced strings, bytes, and cells. The native source-level
factory now tags nursery chunks allocated during its synchronous scope. Its graph still has
to pass exact outgoing closure: oversized, tenured and side allocations remain outside the
candidate. The engine's private weak-key registry now has a single pending request slot per
realm. It assigns a monotonic, realm-local opaque ID, roots only the handle while pending,
resolves that ID against the live registry at escrow, refuses if the registry changed while
pending, and rolls back without allocation. The
request helper is internal to the C runtime; the scheduler and actor mailbox do not yet call
it. No graph address may appear on the handle, ticket, mailbox record, or request sent to the
scheduler. Reject a candidate when allocation falls back to the
ordinary nursery, it contains a finalized block, or any closure edge is outside its chunks.

1. On mailbox grant, reserve a destination inbox entry and the receiver's heap region-set
   capacity. Change the ticket to `pending` and enqueue an engine-owned request containing only
   handle ID, ticket ID, destination realm ID and reservation ID. Do not detach a graph in the
   JavaScript callback. Return to the realm drive loop so every generated frame has unwound.
2. At `drive_one_ready_turn`, before running another language turn or collection, lock the
   pending request and pause both realm mutators. Resolve the opaque handle ID and atomically
   exchange its registry root into one explicit escrow root. Revoke the handle/ticket for the
   duration of proof. No sender JavaScript may run until either rollback or publication.
3. Establish the machine-word scan boundary **before loading a graph pointer into a proof
   frame**. Spill callee-saved registers, then scan from that boundary through the thread's
   stack base, including all older scheduler and sender frames. Failure to determine the stack
   base or capture the required registers refuses. Stale words may cause false refusals. The
   proof's younger frames may be excluded only because they have not run before the boundary;
   no older frame may be skipped. An opaque ID, rather than a pointer argument, crosses this
   boundary. A clear supplied span from `kiln_c99_graph_machine_words_alias` is insufficient.
4. Prove exact outgoing closure, outside-heap edges, C roots, provider and registered roots,
   and the machine span while both mutators stay paused. Exclude only the identified escrow
   slot from the root visitor, never every root equal to the graph pointer. Any unknown edge
   kind, legacy mark-only callback, side owner, scan error or alias refuses. Current incoming
   probe has no accepting result and cannot serve as this certificate.
5. On refusal, restore the escrow root to the same handle table entry, set the ticket to
   `available`, release the destination reservation, and then resume either realm. A retry
   must see the same graph. On success, unlink the complete candidate batch, adopt it into the
   reserved receiver region set, install its root into the reserved destination slot, and only
   then publish the inbox entry. Mark the ticket `consumed` and remove the sender escrow root
   before either mutator resumes. Receiver capacity and all fallible publication resources
   must already be reserved: after sender unlink there can be no rollback by copying pointers
   back to a live sender.

`drive_one_ready_turn` supplies the generated-frame-unwound boundary for the scheduled path.
The unscheduled `drive_async` path must use the same safepoint transaction or refuse graph
requests. The scheduler now has a two-ticket claim gate, but the actor request still does not
name those tickets. Suspending only the sender is insufficient: the receiver could collect
while its region set and inbox root are being updated.

## Receiver reservation and pause ordering

The C runtime now has one native receiver inbox slot. Reserving it allocates any needed region
set capacity and records the receiver collection epoch and region count. Refusal can clear the
reservation without allocation. A low-level helper validates a supported root and complete
candidate closure, calls the existing all-or-nothing chunk mover, then stores the root in that
slot with no allocation or callback between unlink and publication. It accepts a cell's exact
payload pointer or an inline string's actual boundary value, which points to its bytes; a
sliced string root remains refused because its bytes name its parent block. The published slot
is a GC root and cannot be rolled back. It remains occupied until a future actor inbox takes
ownership; there is no delivery path to the JavaScript mailbox yet. This helper is a storage
primitive, not a sender ownership certificate.

`PinnedTurnScheduler::claim_pair` atomically claims two live scheduler realm IDs in ascending
order and wakes both tickets. A lane acknowledges only when it has leased that ticket and before
the next body turn; a turn already running must return first. The coordinator thread waits for
both acknowledgements, then holds an RAII claim that prevents either body from running until
release. Cancellation invalidates the wait and defers body teardown until claim release. The
claim retains the scheduler, so dropping the scheduler cannot tear down paused bodies. No
cross-lane `Rc<NativeRealm>` is borrowed. The hot turn path takes only an atomic read when no
claims exist. A deterministic pinned-plus-movable fixture checks the pause, ordering, refusal
while a turn is still running, cancellation and rollback.

The provider's migrating launcher now registers each live provider realm handle with the
corresponding pause target when `identify` returns. The registration stores only the opaque
realm ID, a weak scheduler reference and scheduler ticket state; it unregisters before the
realm's `finish` callback. Dedicated-thread and legacy pinned launchers have no registration
and refuse lookup. A coordinator can resolve two realm IDs into one scheduler pair claim,
rejecting missing realms, different schedulers and busy targets. A deterministic provider
fixture checks the mapping, pause, release and teardown. No C heap pointer or lane-local
`Rc<NativeRealm>` crosses this registry.

The actor transaction must request the pair **after** the sender's generated callback returns.
It must not wait from a
scheduler lane, which could own either claimed body and deadlock. If either realm exits, cannot
park, or fails reservation, release both claims and restore sender escrow before resuming work.
After both acknowledgements, reserve the receiver's native slot and JS mailbox entry, establish
the sender machine-scan boundary, prove ownership, then perform the C move and native root
publication. The receiver's JS inbox adoption and ticket consumption must be precommitted to
an infallible path before unlink. Release the claim after full rollback or publication. The
current actor request and mailbox do not invoke this scheduler gate.

## Admission gates still missing

- The ticket still owns its handle. The engine's weak-key registry has an opaque-ID pending slot
  and can move its graph value into a reversible escrow root, but no scheduler request or
  reversible ticket state invokes it yet. The C-root visitor can exclude precisely that escrow
  slot for the active ID and still detects a duplicate alias in another slot; this is not a
  complete incoming-root result.
- `isolate(factory)` tags exclusive nursery chunks, but not every possible graph allocation
  uses that nursery. Collection after the factory can invalidate the candidate epoch. The
  outgoing walker still refuses arrays, objects and every unsupported edge or side owner.
- The read-only root scans distinguish a provider alias from incomplete provider callback
  coverage. The opaque-request probe now captures a stack anchor and register image before
  resolving the graph ID, scans older machine words for interior or tagged aliases, and rolls
  escrow back on every result. A focused C fixture proves that a pointer retained in an older
  frame is rejected. It still has no accepting result: generated roots, complete side ownership,
  and the actor-to-scheduler safepoint are not wired into one certificate. The collector's
  marking stack scan remains separate from this read-only check.
- The current actor mailbox grant reserves an application-level slot but does not coordinate it
  with the native receiver slot and region capacity. The two-realm claim/park protocol and JS
  inbox adoption after native root publication are missing from the actor integration; the
  provider mapping is not yet connected to the actor request or mailbox grant.
- Arrays and objects remain excluded until their side allocations, shape keys, local prototype
  state and finalizer ownership have an exact mover and edge visitor.

The native actor fixture asserts that two rejected sends leave the same graph ticket unconsumed.
The backend C fixture asserts that a mark-only provider callback produces a distinct incomplete
result. Neither fixture admits a graph transfer.
