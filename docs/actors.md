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
  API for a graph constructed in exclusive storage. The native runtime must
  prove that the sender has no remaining reference into the graph. A graph that
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
implementation still builds a separate Node and module bootstrap graph for each
actor. On the 2026-09-28 Linux capacity workload, 100 to 1,000 held public
actors added about 816 KiB of RSS per actor, and a warm `spawnActor` took about
2–3 ms to become ready. Use [the capacity benchmark](../bench/concurrency/README.md)
on your deployment machine when choosing actor counts; these figures are a
measured limit of this implementation, not an API limit.

These modules are kiln extensions. `KILN_EXTENSIONS=0` removes them without
changing Node's built-in surface. `@kiln/concurrency` re-exports the actor API
for migration from its former in-realm proxy API.
