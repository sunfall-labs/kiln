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
import { spawnActor, connectActor, transfer } from 'kiln:actors';
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
- `isolate(factory)` and `transfer(handle)` are the explicit object-graph move
  API. A graph that cannot pass the native ownership proof raises
  `DataCloneError` and leaves its ticket usable in the sender. Use ordinary
  cloned arguments or shared snapshots when ownership transfer is unnecessary.

The address is a cloneable call capability. `connectActor(address)` grants
calls; only the `owner` returned by `spawnActor` can stop the actor. Calls from
one reference enter its bounded mailbox in order. A method that awaits work
can overlap another call after its current synchronous segment yields.
`owner.stop()` drains accepted calls; `owner.stop({ force: true })` ends the
actor at a turn boundary.

These modules are kiln extensions. `KILN_EXTENSIONS=0` removes them without
changing Node's built-in surface. `@kiln/concurrency` re-exports the actor API
for migration from its former in-realm proxy API.
