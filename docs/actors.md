# Actors and shared values

`kiln:actors` runs each actor's module in its own realm. Calls are asynchronous,
and arguments normally cross the realm boundary by structured clone. Put the
actor class in a module that the build can find:

```js
// counter.mjs
import { Actor } from 'kiln:actors';

export class Counter extends Actor {
  constructor(start) {
    super();
    this.count = start;
  }

  increment(delta) {
    this.count += delta;
    return this.count;
  }

  firstByte(buffer) {
    return new Uint8Array(buffer)[0];
  }

  readOwner(record) {
    return record.owner;
  }
}
```

Spawn it from the calling module. `module` must be a URL or absolute path;
`new URL` resolves a relative file at the call site.

```js
// main.mjs
import { spawnActor, connectActor, prepareTransfer, transfer } from 'kiln:actors';
import { share } from 'kiln:shared';

const owner = await spawnActor({
  module: new URL('./counter.mjs', import.meta.url),
  export: 'Counter',
  args: [4],
  mailboxCapacity: 8,
});

try {
  const ref = connectActor(structuredClone(owner.ref.address));
  console.log(await ref.call('increment', [3])); // 7

  const buffer = new Uint8Array([23]).buffer;
  console.log(await ref.call('firstByte', [transfer(buffer)])); // 23
  console.log(buffer.byteLength); // 0: the accepted call moved its backing store

  const record = share({ owner: 'Ada' });
  console.log(await ref.call('readOwner', [record])); // Ada

  const source = { owner: 'Grace' };
  const snapshot = prepareTransfer(source);
  source.owner = 'Changed';
  console.log(await ref.call('readOwner', [transfer(snapshot)])); // Grace
} finally {
  await owner.stop();
}
```

Run the interpreter with `kiln --node run main.mjs`, or compile and run the
native path with `kiln --node run --native main.mjs`. `--node` enables the
worker messaging adapter. A native build must be able to find the actor module
in its source graph.

## Passing data

- Plain arguments and results are cloned. A raw `ArrayBuffer` is copied.
- `transfer(buffer)` moves an `ArrayBuffer` when the call is accepted. A call
  refused before acceptance leaves the buffer attached.
- `share(value)` makes a deeply immutable snapshot of supported plain objects
  and arrays. Later sends retain that shared graph without copying its fields.
  The source object remains independent. See [ADR 0073](adr/0073-user-shared-values-are-frozen-graphs.md)
  for accepted values and property rules.
- `prepareTransfer(value)` snapshots a supported plain object or array graph
  into private storage. The original and its aliases remain usable. Passing
  `transfer(handle)` to an actor moves the snapshot once; preparation copies the
  graph, while the native handoff moves its storage without copying the payload
  again. Cycles and repeated references retain their identity within the
  snapshot. Array holes remain absent, and ordinary objects keep numeric own
  keys in JavaScript enumeration order. Accessors, non-default property descriptors
  and unsupported host objects raise `DataCloneError` during preparation.
- `isolate(factory)` and `transfer(handle)` are the explicit object-graph move
  API for a graph constructed in exclusive storage. Both engines prove that
  the sender has no remaining reference into the graph. Native moves certified
  heap chunks without copying; the interpreter encodes and reconstructs the
  verified graph because its ordinary objects cannot cross threads. A graph that
  cannot pass that proof raises `DataCloneError` and leaves its ticket usable in
  the sender. Use `prepareTransfer` when the source must remain usable, or
  ordinary cloned arguments when a separate copy is sufficient.

The address is a cloneable call capability. `connectActor(address)` grants
calls; only the `owner` returned by `spawnActor` can stop the actor. Calls from
one reference enter its bounded mailbox in order. A method that awaits work
can overlap another call after its current synchronous segment yields.
`owner.stop()` drains accepted calls; `owner.stop({ force: true })` ends the
actor at a turn boundary.

Native actors share a bounded realm scheduler. Their mailbox and asynchronous
methods release a lane between turns. `Atomics.wait` throws `TypeError` inside
an actor because a synchronous wait could occupy the lane needed by its
notifier; use actor messages for coordination. Ordinary `node:worker_threads`
retain dedicated threads and their synchronous `Atomics.wait` behavior.

## Capacity today

The scheduler uses a bounded set of lanes for asynchronous actors, so keeping
more actors alive does not require one OS thread per actor. The current native
implementation still builds most of the Node and module bootstrap graph for each
actor. In the 2026-09-29 Linux capacity workload, 100 to 1,000 held public
actors added about 530 KiB of RSS per actor after the process environment layout
was shared; a warm `spawnActor` took about 2–3 ms to become ready. The earlier
10,000-actor capacity run used a version that retained more memory per actor.
Use [the capacity benchmark](../bench/concurrency/README.md) on your deployment
machine when choosing actor counts; these figures are measured limits of this
implementation, not API limits.

## Errors

A call rejects with one of:

- `ActorRemoteError` -- the method threw. `remoteName` is the name of what it
  threw, and `cause` a clone of its cause when that could be cloned.
- `ActorClosedError` -- the actor was stopped or its realm ended (an uncaught
  error, `process.exit`), including for calls already in flight.
- `DOMException` named `DataCloneError` -- an argument or result could not
  cross, as `structuredClone` would refuse it. The actor keeps running.
- the signal's reason, when the call's `AbortSignal` aborts it.

Both error classes are exported from `kiln:actors`, so `error instanceof
ActorClosedError` works. A constructor's arguments travel like a call's, and a
constructor that throws rejects `spawnActor` with `ActorRemoteError`.

## Types

`@kiln/types` declares `kiln:actors` and kiln's other modules; add it to
`compilerOptions.types`. `ActorRef<T>.call` accepts only `T`'s public methods
with their argument types and resolves to their awaited results.

## Using an actor from Effect

An actor is a scoped resource: acquire it with `spawnActor` and release it with
`owner.stop()`. No adapter package is needed.

```ts
import { Effect, Layer, Context } from "effect";
import { spawnActor, type ActorRef } from "kiln:actors";
import type { Counter } from "./counter.ts";

class Counters extends Context.Service<Counters, ActorRef<Counter>>()("Counters") {}

const CountersLive = Layer.effect(
  Counters,
  Effect.acquireRelease(
    Effect.promise(() => spawnActor<Counter>({
      module: new URL("./counter.ts", import.meta.url), export: "Counter", args: [0],
    })),
    (owner) => Effect.promise(() => owner.stop()),
  ).pipe(Effect.map((owner) => owner.ref)),
);
```

These modules are kiln extensions. `KILN_EXTENSIONS=0` removes them without
changing Node's built-in surface.
