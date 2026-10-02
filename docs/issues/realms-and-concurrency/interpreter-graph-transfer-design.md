# Interpreter graph transfer bridge

Status: implemented for plain graphs through the shared actor reservation protocol. The
remaining layout and performance work is tracked in realms issue 10.

The native runtime can relink certified nursery chunks between realm heaps. The interpreter's
ordinary objects and arrays use `Rc` and `RefCell`, so sending either an `Rc` or a raw pointer to
another worker thread would violate their ownership contract. Replacing every interpreter value
with a synchronized representation would also put locking on ordinary single-realm execution.
The interpreter can preserve the public move's observable semantics by proving exclusivity in
the sender turn, encoding the closed plain graph into the existing `Send` message-token format,
and letting the receiver reconstruct it. This is an interpreter implementation cost; native
direct transfer remains the zero-copy implementation required by ADR 0070.

## Sender certificate

Only the plain object and array layouts already accepted by `InterpreterPlainSource` qualify.
Before using this for transfer, its inspection must additionally reject every hidden field that
can retain a value or expose the object later: non-default descriptors, exotic array elements,
custom prototypes, host state, namespace cells, finalization state and weak observers. A rejected
graph leaves the transfer ticket unconsumed.

The plain encoder's handle vector has one clone of the root and one clone for every traversed
object or array edge, including back edges and repeated references. For each distinct mutable
node, let `h` be its occurrence count in that vector. A closed graph with one root held in the
isolated-handle escrow has exactly `2h` strong `Rc` owners: `h` encoder handles, `h - 1` graph
edges plus the escrow root for the root node, or `h` graph edges for any other node. Require the
observed strong count to equal `2h` and the weak count to be zero. The proof runs after a
successful side-effect-free encode, before creating any decoded value, with no JavaScript
checkpoint in between. Any extra VM local, global, closure, second handle, weak observer or
unsupported edge refuses the move. Tests must exercise cycles, repeated edges, aliases and
weak observers before this certificate is used in production.

## Delivery transaction

Both engines use the same actor message, opaque reservation, sender claim, publication lease,
receiver authentication, release, and ticket status state machine. The receiver holds a
placeholder while publication is pending. A failed proof or cancelled reservation restores the
sender ticket; published status consumes it. The two engines supply different payloads under
that protocol:

- Native relinks certified collector chunks while both realm turns are claimed. Its receiver
  gets the adopted root from the native inbox.
- The interpreter proves the `Rc` graph exclusive, encodes it as thread-safe plain tokens,
  stores those tokens in the host reservation, and drops the sender's escrow root before
  publishing. The authenticated receiver decodes the tokens and assigns the existing slot.
  The reservation keeps the wire until release so a failed assignment can retry.

The interpreter copy costs time proportional to graph size. Benchmark that cost separately.
Both engines preserve identity and cycles in the received graph, refuse retained aliases,
and consume the ticket only at their irrevocable publication point. There is no claim that
interpreter `Rc` blocks move between threads without copying.

## Required probes

- Successful plain object and array graphs, cycles, repeated references, numeric keys and holes.
- A global, captured local, second isolated handle and weak reference retaining any node refuse
  without consuming the ticket.
- A failed post, cancellation before acceptance and an actor closed before acceptance leave
  the graph usable in the sender; a successful accepted call consumes exactly once.
- Interpreter and native results match on the public cases; native still passes stressed GC and
  simultaneous-realm probes. A large-graph benchmark distinguishes interpreter copy cost from
  native relink cost.
