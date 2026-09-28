# ADR 0072: Actors are module-addressed realms

## Status

Accepted, 2026-09-27, under the project owner's delegation to adjust or approve the API
contracts and implement them.

Specifies the actor API left open by ADR 0070. Uses its realm scheduler, messages, shared
immutable region and ownership transfer; follows ADR 0068 for kiln extensions. Supersedes the
actor API sketch in the now-superseded ADR 0057. The existing `@kiln/concurrency` implementation
does not satisfy this contract.

## Context

An actor should be cheap enough to own one connection or tenant, and its mutable state must live
in its own realm. `@kiln/concurrency` currently constructs the class in the caller's realm and
returns an asynchronous `Proxy`. Its `isolate` and `transfer` helpers return the same object. A
typed proxy makes the call look remote without giving it isolation, a mailbox or ownership
semantics.

Issue 10 has transferred `ArrayBuffer` backing stores and adopted off-heap worker arenas. A
general object graph still cannot move: the sender's nursery chunks mix unrelated allocations,
and no check proves the sender has released every alias. Issue 10 asks issue 12 to define the
consuming operation; issue 12 had in turn been marked blocked by issue 10. The API can be decided
before the zero-copy implementation as long as an unproved graph move fails explicitly.

Passing a class constructor to `spawnActor` is also the wrong realm boundary. A constructor may
capture arbitrary functions and objects from its defining realm. Neither structured clone nor a
new realm can reproduce those captures. An actor must instead name code the destination can load
for itself.

## Decision

### Creation and authority

The engine owns a kiln extension module, `kiln:actors`, with this public shape. The TypeScript
declarations use ordinary TypeScript and do not require a patched compiler.

```ts
import { Actor, spawnActor, connectActor, isolate, transfer } from "kiln:actors";
import type { Counter } from "./counter.js";

// counter.ts exports class Counter extends Actor.
const owner = await spawnActor<Counter>({
  module: "./counter.js",
  export: "Counter",
  args: [initialCount],
  mailboxCapacity: 64,
});
const count = await owner.ref.call("increment", [5]);
const address = owner.ref.address;
await owner.stop();
```

The relevant type boundary is deliberately small:

```ts
type Methods<T> = {
  [K in keyof T & string]: T[K] extends (...args: any[]) => any ? K : never
}[keyof T & string];

interface ActorRef<T extends Actor> {
  readonly address: ActorAddress<T>;
  call<K extends Methods<T>>(
    method: K,
    args: T[K] extends (...args: infer A) => any ? A : never,
    options?: { signal?: AbortSignal },
  ): Promise<T[K] extends (...args: any[]) => infer R ? Awaited<R> : never>;
}

interface ActorOwner<T extends Actor> {
  readonly ref: ActorRef<T>;
  stop(options?: { force?: boolean }): Promise<void>;
}
```

`spawnActor<T>` returns `Promise<ActorOwner<T>>`. It resolves only after a new realm has loaded
the module, selected the named export, constructed one `Actor` subclass instance and installed its
mailbox. A constructor exception rejects the spawn and destroys that realm. `T` describes the
instance API for TypeScript; it is not a runtime ownership proof. The export must be a class
extending the engine's `Actor` base. The runtime validates this and the public method named by
every call. It does not invoke getters, setters, `constructor`, `Object.prototype` members or
private methods as actor calls.

`module` is a `URL` or module specifier string resolved relative to the calling module with the
same resolution policy as an ordinary import. `export` is mandatory, including for `default`.
Native closed-world builds include statically named actor modules in their module graph; a
runtime-only target absent from that graph refuses at spawn with a named module-not-linked error.
The interpreter applies its ordinary module resolver. No source text, function, closure or class
object crosses the boundary. Constructor arguments use the same message rules as calls, including
the explicit transfer forms below.

An `ActorRef<T>` has `address` and `call(method, args, options?)`. A call is always asynchronous,
including when the caller and actor happen to run on the same scheduler thread. Its type permits
only public callable string keys of `T`, with the corresponding argument tuple and awaited result
type. The actor's object, fields and methods are never returned to the caller. The method lookup
and invocation happen in the actor's realm. `ActorAddress<T>` is an opaque, structured-cloneable
call capability, valid only within its hosting process; it has no string representation that can
be forged. `connectActor<T>(address)` makes a new reference from it. Sending the address grants
call authority, not shutdown authority. Only the non-cloneable `ActorOwner<T>` returned by spawn
can stop the actor. A process boundary invalidates the address rather than silently creating a
different actor.

The actor realm receives the spawning program's host capabilities, subject to the same host
policy used for `node:worker_threads`; this API does not create stronger capabilities. An actor
has its own global object, heap, module state, clock turn and microtask queue. Its process-wide
immutable data is shared under ADR 0070, but its mutable objects are never directly shared.

### Calls and lifecycle

Each `call` is one request message with a reply ID. Arguments are structured-cloned into the
actor realm, except explicit `transfer(buffer)` tickets for `ArrayBuffer`, shared immutable
values (when issue 11 defines them), and proven ownership moves. `transfer` is overloaded for
`ArrayBuffer` and `Isolated<T>` handles; a raw `ArrayBuffer` without a ticket is copied.
`SharedArrayBuffer` retains its usual shared-bytes semantics, never shared-object semantics.
Results take the same path back. An uncloneable argument or
result rejects that call with `DataCloneError` and leaves the actor running. A thrown or rejected
actor method rejects the caller with an `ActorRemoteError` containing the remote error's name,
message and cloneable cause; it does not expose the remote `Error` object's identity. A method
may explicitly return a transfer ticket, subject to the same proof at the actor's safepoint.

The mailbox is bounded. `mailboxCapacity` is a positive integer, defaulting to 64, and counts
accepted requests waiting to start. A call that finds it full waits for capacity; it does not
retain an unbounded encoded payload. The argument snapshot and any consuming transfer occur
when capacity is reserved, not when the caller first creates the Promise. Waiters from one
reference reserve capacity in call order. Accepted calls from one reference enter the mailbox in
order; calls from different references merge in arrival order, which deterministic scheduler
mode makes reproducible. Every synchronous segment runs alone in the actor realm. An `await`
yields its turn, so another call may start before the first Promise settles; code must not assume
method-wide atomicity across `await`.

`call` may take a caller-local `AbortSignal`. Aborting before acceptance removes the waiter and
leaves its arguments and transfer tickets untouched. Aborting after acceptance rejects the
caller's wait, but does not roll back a message or preempt an actor method. The actor may still
finish it. Actor shutdown is explicit: `owner.stop()` refuses new calls, drains accepted calls and
resolves after the realm exits. `owner.stop({ force: true })` rejects queued and outstanding calls
and ends the realm at the next turn boundary; a running synchronous segment is not preempted.
Addresses and references to a stopped actor reject future calls with `ActorClosedError`.
An unexpected realm exit rejects every outstanding call with that error. Dropping a reference
does not silently kill a live actor; the owner is responsible for its lifetime.

### Isolation and consumption

`isolate(factory)` returns an opaque `Isolated<T>` handle, not `T` with a phantom brand. The
factory runs in the sender realm and can build a candidate graph in dedicated allocation chunks
once issue 10 supplies them. The handle is the sole *API-level* route to the result; the type
system cannot establish that the factory did not leak an alias. The old `isolate(value)` overload
is retired: wrapping an already allocated value cannot give its mixed nursery chunks exclusive
ownership. `transfer(handle)` creates a single-use `TransferTicket<T>` accepted only as an
argument or result of an actor call (and later by other explicit message APIs). Merely calling
`transfer` neither moves the graph nor invalidates the handle. `transfer(buffer)` likewise
defers detachment until the message is accepted; a failed acceptance leaves the buffer attached.
No implementation may treat a
TypeScript brand, an identifier's spelling or a package hook as proof of ownership.

At a sender turn safepoint, after the generated frame has unwound, the runtime has reserved
mailbox and destination capacity and examines the actual graph and every sender root. Let `G`
be its transitive closure through properties, elements, prototypes, internal slots and closure
environments. Every edge leaving `G` must point to an object in the shared immutable region with
valid identity in the destination. No sender root or live object outside `G` may point into `G`
after the consuming handle is excluded. This includes stack/register roots, module bindings,
queued jobs, other message arguments, host and provider handles, weak collections and finalizer
state. Unknown edge and root kinds make the proof fail. A conservative root may cause a safe
move to fail, but may not make an unsafe one pass.

The blocks containing `G` must belong exclusively to its dedicated chunks. Issue 10 must add
sender unlink to pair with existing destination adoption, or evacuate `G` into exclusive chunks
first (which copies and is not reported as zero-copy). No unrelated live allocation, pending
finalizer or realm-specific mutable prototype may ride along. With both proof and storage
conditions satisfied, the runtime removes the consuming sender root, unlinks the chunks, adopts
them in the destination and publishes the message as one transaction with no mutator or
collection between those actions. Reservation or proof failure leaves the sender graph and ticket
usable; a successful publish consumes the ticket and invalidates the sender handle. A later
actor failure does not restore ownership to the sender.

Until issue 10 implements that proof and handoff, a graph ticket raises `DataCloneError` without
consuming it. Ordinary structured-cloned calls and existing `ArrayBuffer` transfers can ship
first. There is no identity-helper fallback and no automatic clone of a failed ownership move:
cloning would leave aliases usable and silently break consumption. The zero-copy claim applies
only to the proven exclusive-chunk handoff, not to graph construction, evacuation or arbitrary
objects.

### Module and package boundary

`kiln:actors` is the authoritative engine-owned extension. `KILN_EXTENSIONS=0` and
`kiln build --no-kiln-extensions` remove it along with other kiln extensions (ADR 0068), with a
named unavailable-module error on import. No Node built-in is patched and no ordinary Node-only
program changes behavior.

`@kiln/concurrency` remains as a compatibility import that re-exports `kiln:actors` and its
types, and no longer constructs an in-realm proxy or exports identity `isolate`/`transfer`
helpers. Its `parallelMap` prototype fallback is split out or retired; the engine's built-in
definition remains the single owner under ADR 0068. The package may offer a separate, explicitly
named Node adapter using `worker_threads` and structured clone, but it must not claim graph
ownership transfer or pretend to be the kiln actor runtime. Existing users of
`spawnActor(Constructor)`, method-proxy calls, `isolate(value)` and identity `transfer` receive
migration errors rather than behavior that looks remote but is local.

## Implementation order

1. Publish the `kiln:actors` module contract and types; implement module-addressed spawn, opaque
   addresses, bounded mailboxes, calls and lifecycle using structured clone and current
   `ArrayBuffer` transfer. Reuse the realm scheduler from issue 09; until it lands, pinned realms
   can preserve the same semantics without claiming the final density target.
2. Make `@kiln/concurrency` a thin re-export, replace its old tests with real cross-realm tests,
   and document the source break. Test both engines and the extension-off mode.
3. Implement issue 10's dedicated-chunk allocation, exact root/edge proof, sender unlink and
   atomic handoff behind `isolate`/`transfer`; include conservative rejection probes and
   `native-stress`. This order resolves the issue 10/12 dependency cycle: the API no longer
   waits for the optimized move, and the move has a concrete consuming operation to implement.
4. Measure actor startup, incremental memory, mailbox latency and transfer sizes. The
   per-connection/per-tenant density target remains gated by issue 08's realm memory work and
   issue 09's scheduler, independent of API correctness.

## Rejected alternatives

**`spawnActor(Constructor)` with an asynchronous method proxy.** It cannot move a closure or its
module environment into a new realm, and property access hides message, failure and shutdown
boundaries. The current package implementation proves that this spelling can silently remain
in-realm.

**Make addresses strings or allow every address to stop its actor.** Strings are forgeable, and
call authority should not include lifecycle authority when an address is sent to another realm.

**Treat `Isolated<T>` as a static uniqueness proof.** Standard TypeScript has aliases and does
not enforce consumption. The runtime must prove the graph against its complete root set.

**Clone when an ownership move fails.** That would deliver a value while leaving sender aliases
live, contradicting the consuming API and concealing the missing zero-copy path.
