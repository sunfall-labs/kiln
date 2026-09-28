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

`isolate` now takes a factory and returns a handle. `transfer` returns a ticket consumed when a
message is accepted. `ArrayBuffer` tickets move the buffer; object graph tickets currently reject
with `DataCloneError` until Kiln can prove sole ownership. There is no identity fallback.

The package includes ambient TypeScript declarations for `kiln:actors`. Direct imports can add
`@kiln/concurrency` to `compilerOptions.types`; package imports get the declarations through
`index.d.ts`. The package does not install `parallelMap` methods. Kiln owns those built-ins.
