# @kiln/concurrency

This package re-exports Kiln's `kiln:actors` extension. An actor runs its class in a separate
realm and receives calls through a bounded mailbox. Run programs with Kiln's Node adapter;
`kiln:actors` is unavailable when Kiln extensions are disabled.

```js
import { Actor, spawnActor } from "@kiln/concurrency";

// counter.js exports `class Counter extends Actor`.
const owner = await spawnActor({
  module: new URL("./counter.js", import.meta.url),
  export: "Counter",
  args: [0],
});
console.log(await owner.ref.call("increment", [1]));
await owner.stop();
```

The old `spawnActor(Constructor)` and method proxy have been removed. Name a module and export,
then call methods through `owner.ref.call(name, args)`. `owner.ref.address` can be cloned into
another realm and passed to `connectActor`; only the owner can stop the actor. Relative module
paths need `new URL(path, import.meta.url)` so they resolve at the call site.

`prepareTransfer(value)` snapshots a supported plain data graph and leaves `value` usable.
`transfer(handle)` makes a ticket consumed when an actor accepts that snapshot. Native Kiln
prepares the copy in a private arena and moves its storage to the actor; the interpreter keeps
a private snapshot and clones it into the accepted message. Arrays, ordinary objects
with computed string keys, cycles, strings, numbers, booleans, `null` and `undefined` are
supported. A null-prototype source is accepted and, as with structured clone, becomes an
ordinary object in the receiver. Array holes are preserved. Accessors, symbol keys, other custom
prototypes and host objects raise `DataCloneError`; native Kiln can still refuse storage it
cannot move.
One graph ticket is accepted per call.

`isolate(factory)` is the separate direct ownership path. Native Kiln moves it only after
proving that no sender alias remains; the interpreter currently refuses direct graph moves.
`transfer(buffer)` moves an `ArrayBuffer` without copying its bytes. A failed acceptance keeps
the ticket and source usable.

The package includes ambient TypeScript declarations for `kiln:actors`. Direct imports can add
`@kiln/concurrency` to `compilerOptions.types`; package imports get the declarations through
`index.d.ts`. The package does not install `parallelMap` methods. Kiln owns those built-ins.
