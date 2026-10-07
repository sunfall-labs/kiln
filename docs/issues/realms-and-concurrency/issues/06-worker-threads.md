# 06. `node:worker_threads` on realms

Status: resolved (pinned deploy fixtures match Node in both engines; see remaining risks)
Blocked by: 05

Implement `node:worker_threads` with Node's semantics, a worker being a realm (ADR 0070):
`Worker` (from a file, from `eval`, with `workerData`, `env`, `resourceLimits` as far as they apply),
`parentPort`, `isMainThread`, `threadId`, `MessageChannel`, `MessagePort`, `BroadcastChannel`, the
structured clone algorithm, transfer lists (copying at first; 10 makes them zero-copy),
`SharedArrayBuffer` and `Atomics` across realms, `worker.terminate()`, and error and exit events.
What `process` looks like inside a worker follows Node's rules for workers.

Written once in TypeScript over declared primitives (ADR 0065), with the realm and port machinery in
the runtime.

## Acceptance

- The deploy corpus's pino-with-transport case and a piscina case match Node.
- Node's own `worker_threads` behaviours covered by probes in `conformance/node`, run with `--node`.

## Comments

### 2026-09-28 -- register cross-port exit listener before awaiting the port reply

The `worker-threads-messages` probe awaited a transferred port's reply before registering
its `exit` listener. Those events travel on separate ports, so under concurrent native GC
stress the worker could exit first and leave the later promise pending while the process
finished with no output. The probe now registers both waits before sending the port. Its
native output matched the interpreter in 32 concurrent stressed runs. The full native stress
gate then reported 304 matches, one known refusal, no divergences and 39 declared skips.

### 2026-09-27 -- `node:worker_threads` in both engines (branch `work/worker-threads`)

**Shape.** One runtime, one TypeScript, two thin engine seams.

- *Runtime* (`kiln-runtime/src/workers.rs`, process-wide, `Send`): endpoints (a queue, a `Notify`,
  peer-closed state), channels, broadcast groups, `enqueue` for a port's own queue; workers
  (`spawn` on a thread of the process's realm host -- `WorkerPool::spawn_pinned_with_stack`, 64 MB
  stack -- `terminate`, `reap`, `finish_children`, per-program `threadId`s from 1 with a separate
  process-unique key); the `Atomics.wait`/`notify` waiter list keyed by (shared store, byte offset),
  compared under its lock. A message is `{ kind, tokens }`; a token is a primitive, a
  `StringUnits`, a moved `ArrayBuffer` store (`Box<dyn Any>` -- primitive C: the block moves, it is
  never copied), copied bytes (broadcast), or a shared store (`Arc<dyn Any>`). The runtime posts
  `online`, the worker's stdio, an uncaught error and `exit` into the parent's end of the worker's
  channel, in order with the worker's own messages; that end survives the worker closing
  `parentPort` until `exit` is in. New `AsyncHostOp::PortWait { port }` (TokioHost: a task awaiting
  the endpoint's `Notify`), so a port's wait is an ordinary referenced/unreferenced host op and
  keeps a realm alive exactly while it should.
- *TypeScript*: `web_events_streams.ts` owns messaging. A `MessagePort` whose other end is in
  another realm is bound to an endpoint (`#remote`) and takes messages from the runtime as it
  dispatches them, through the same delivery loop and checkpoints as an in-realm port. The wire is
  the one structured clone (`structured_clone.ts`, unchanged in scope) followed by a flattening of
  its fresh copy into tokens (tags + payload, back-references for shared objects and cycles) --
  a wire format over a value the clone already made, not a second clone, and the proof ADR 0070
  asks before a store moves. Ports transfer between realms (a local partner is rebound to a new
  channel; queued messages go with the port); `receiveMessageOnPort` takes across realms;
  `BroadcastChannel` is a runtime group (in-realm peers are kicked in the posting turn, as node
  delivers). A port with `message` listeners refs its realm, the last one removed unrefs it
  (node's `once` behaviour); `MessagePort` gained node's `on/once/off/emit/...`. The realm
  bootstrap adopts a worker realm (`OP_SELF`): `parentPort`, `workerData`/`env`/environment data
  from the INIT message, the unref'd control port `terminate` wakes, `process.stdout/stderr._write`
  to the parent, and node's `ERR_WORKER_UNSUPPORTED_OPERATION` for `chdir`, `abort`, `set*id`.
  `node/worker_threads.ts` is node's `Worker` over that (validation and messages as node's,
  `argv`, `execArgv` accepted and ignored, `env`/`SHARE_ENV`, `resourceLimits` reported with node's
  defaults -- and -1 for unset ones on the parent side, as node reports -- `name`/`threadName`,
  `stdout`/`stderr` options, `ref`/`unref`, `terminate()` resolving the code, `threadId` -1 after
  exit), plus `isInternalThread`, `threadName`, `postMessageToThread` (refuses).
- *Interpreter*: `Native::WorkerOpRaw`/`PortWaitRaw` (`vm/workers.rs`); `ExecutionCapabilities::
  with_worker_host` -- module-eval installs `GraphWorkers` for every Node-mode graph run: a worker
  runs `graph.with_entry(path).execute_node` on its thread (eval text: a graph built at run time
  around a virtual `[worker eval].cjs` entry, `ModuleGraph::build_node_virtual`), its stdout/stderr
  forwarded, the uncaught value (now recorded for event-loop and unhandled-rejection failures too)
  encoded by `worker_error_tokens`. Termination is checked every 100k instructions in a worker.
  `Atomics` RMWs on a shared buffer hold one lock across read and write (they lost updates across
  threads), and `wait`/`notify` use the runtime list whenever workers exist. Fixed on the way: a
  module's `process` was a different object from the realm's global one in a second machine.
- *Native*: method ids 539 `__kilnWorkerOp`, 540 `__kilnPortWait`, 541 `__kilnSharedBuffer`, 542
  `__kilnAtomics`; provider ABI 46 -> 47 (138 exports: `kiln_worker_{post,take,message_free,op,
  broadcast_join,set_runner,spawn,self,stdio,exit,exit_code}`, `kiln_shared_{create,release,grow}`,
  `kiln_atomics_{wait,notify}`); `KILN_REALM_OP_PORT_WAIT` 16; native runtime version 13 -> 14
  (`kiln_cli_host_state.error_output`); `kiln_typed_buffer` gained `shared`, `shared_capacity`,
  `shared_growable`. The graph launcher registers a runner; a worker realm runs
  `kiln_cli_run_realm_as(..., worker_entry)` (one module and its imports, found by project-relative
  path so the launcher stays root-independent), its stdio through `fopencookie`/`funopen` streams,
  `process.exit` recording the code and unwinding with the host-error status, an uncaught error
  reported from inside the realm (`kiln_c99_worker_report_error`), terminate at every turn boundary.
  `SharedArrayBuffer` and `Atomics` exist natively now: the constructor is `arraybuffer.ts`'s
  (registered through `__kilnSharedBuffer`), members, `instanceof`, `getPrototypeOf` and the tag
  answer for a shared buffer, and `atomics.ts` (native only -- the interpreter's `Atomics` is its
  own) makes every check over one element primitive (`__atomic_*`, seq-cst).
- *Discovery*: `new Worker(...)` targets are lazy graph edges (`NodeRuntimeRequest`): literal
  paths, `new URL(lit, import.meta.url)`, `path.join/resolve(__dirname, ...)`, `fileURLToPath`,
  through `const` bindings, `||`/`??`/conditionals and a compiler's `(0, path.resolve)(...)` --
  enough for thread-stream's and piscina's own worker scripts.

**Results** (release CLI, final tree). Regress: 35/35 (nine new, r27-r35, node 26.8.1 output).
`conformance/node --node`: 36/36. `scripts/native.sh`: 265 matched, 1 refused (shadow-realm,
pre-existing), 0 diverged -- three new probes `worker-threads-{messages,shared-memory,broadcast}.cjs`
(also identical to node, stable over 20 runs each, clean under `KILN_GC_STRESS=1`). Crate tests:
kiln-runtime 162, kiln-interpreter 197, kiln-backend-c 188 + emitted_c 36 (ABI snapshot updated,
prelude band 53,500 -> 54,600 lines: +~600), kiln-native 221, provider 37, contract 11+5,
native-runtime 16, builtins 98, frontend 49, module-graph 157.

**Measured** (`bench/concurrency/run.sh --only start,messages --quick`, which now runs kiln's
workers; load ~12 on 16 threads). Native: worker start warm 7.2 ms (node 40.5, bun 27.6), cold
8.9 ms; RSS per worker 2.5 / 0.92 / 0.48 MB at 1 / 10 / 100 alive (node 12.2 / 7.4 / 4.4).
Messages: ping-pong 108 us (node 20.4), one-way 64.5k/s (node 499k/s), 1 MB graph 78 ms (node
5.5), 16 MB copied 7.8 ms (node 30.7), transferred 1.8 ms (node 0.04). Interpreter (wall, self-
contained scripts): start 11.5 ms per worker (node 25.8), ping-pong 1.49 ms (node 33 us).

**Left / risks.**
- Acceptance: the deploy corpus has no pino-transport or piscina case yet. Tried by hand: both
  start their worker scripts now, then load the *user's* module by a run-time specifier (pino's
  `target`, piscina's `filename`), which the closed world does not contain -- deploy issue 06
  (closed-world policy). The `worker_threads` probes are in `conformance/regress` and
  `conformance/native`, not `conformance/node`.
- A native worker in a loop that never returns to its event loop cannot be terminated (no
  safepoint in emitted code); it ends at its next turn boundary. The interpreter stops it within
  100k instructions.
- Eval workers are interpreter-only; natively the parent's `error` event says why.
- Messaging speed across realms is 5x (ping-pong) to 14x (graph) node's: every message is an
  interpreted-TS token encode + decode and a host-op round trip through Tokio. The graph row is the
  encoder; a C/Rust flattening of the clone's output, or primitive C moving the clone's blocks
  (issue 10), is the fix.
- `Atomics.waitAsync` across realms: interpreter still times out only; native refuses by name.
  `structuredClone(sab) === sab` is true in both engines (node: a new object over the same memory).
  `SHARE_ENV` does not share later writes. `resourceLimits` are reported, not enforced.
  `Worker.performance.eventLoopUtilization`, `getHeapSnapshot`, `postMessageToThread`,
  `markAsUncloneable`, `moveMessagePortToContext` refuse.
- A worker's stdout is delivered to the parent in order with its messages (node's is an
  independent stream and can arrive later).
- Method ids 539-542 and provider ABI 47 will collide with any concurrent branch appending ids or
  exports.

### 2026-09-27 -- deploy acceptance follow-up (branch `work/worker-deploy`)

Two `conformance/node` probes now exercise worker lifecycle/order and ArrayBuffer transfer under
the Node host adapter; both match Node. Pinned Node package reproductions also pass: pino 10.3.1
with `pino.transport({ target: 'pino/file' })` emits a JSON line, and piscina 5.3.2 runs a task
from `filename: path.join(__dirname, 'piscina-task.cjs')` and returns 42.

The package load path confirms the closed-world gap: pino's transport calls thread-stream 4.2.0,
whose worker loads `workerData.filename` as a file URL; piscina's own worker receives a runtime
`filename` and first calls `require(filename)`. `package.json` `kiln.include` is being implemented
as a declared lazy graph root under deploy issue 16. A module in that list is compiled and retained
but not evaluated by the parent. The deploy app corpus and runner from deploy issue 02 are still
absent, so the two package reproductions are Node oracles, not a kiln deploy acceptance verdict.

The pinned, reproducible app fixture now lives in `conformance/apps/worker-deploy/` (pino 10.3.1,
thread-stream 4.2.0, piscina 5.3.2; `npm ci --ignore-scripts`). On Node 26.8.1, `node pino.cjs`
prints `{"level":30,"answer":42,"msg":"worker transport"}` and `node piscina.cjs` prints `42`.
After declared lazy roots, the worker's Node builtin and JSON dependencies, `process.execArgv`,
and callback `fs.open`/`write`/`fsync`/`close` were wired, `kiln run --one-shot --node pino.cjs`
exits 0 with exactly the same one JSON line. The focused `kiln/fs/descriptor-callbacks` Node
probe prints `pass` in Node and the kiln interpreter. A native fixture starts an argv-selected
declared worker only on demand, gets its message, and catches `MODULE_NOT_FOUND` for an undeclared
sibling; it passes with `KILN_GC_STRESS=1`.

Native pino still refuses at `thread-stream/index.js`: `new FinalizationRegistry` says `new on a
callee that is not a generated function`. Piscina still fails in both engines at
`class Piscina extends EventEmitterAsyncResource`: interpreter says `class heritage does not have a
valid prototype property`, native says `class heritage is not a constructor or null`. The Node
API needs an EventEmitterAsyncResource with an underlying AsyncResource and `asyncId()`; an alias
to EventEmitter would hide the missing async context. Issue 06 stays in progress until both pinned
apps pass interpreter and native. The broader deploy app runner remains issue 02.

### 2026-09-27 -- pinned deploy acceptance (branch `work/worker-acceptance`)

After `npm ci --ignore-scripts`, the pinned `pino.cjs` prints
`{"level":30,"answer":42,"msg":"worker transport"}` and `piscina.cjs` prints `42` with exit
status 0 in Node 26.8.1, the kiln interpreter, and the native executable. The focused native
probes for negative atomic stores, `EventEmitterAsyncResource`, weak references, and file
descriptors match the interpreter and Node; all four also pass with `KILN_GC_STRESS=1`.

The native transport needed TypeScript `WeakRef`/`FinalizationRegistry`, descriptor methods on
`node:fs`, and `Atomics.waitAsync` to observe shared state from later turns. Piscina needed
`EventEmitterAsyncResource` and lazy CommonJS `require` evaluation, including for an optional
native dependency in an untaken branch. The pino shutdown hang exposed a prior native atomic
conversion bug: storing `-1` in a shared `Int32Array` returned `-1` but stored `0`; the end
marker now survives.

Native `Atomics.waitAsync` currently polls shared values every 10 ms, so a notify without a
value change can be missed. Native weak references still hold their targets and do not schedule
finalizer callbacks, matching the interpreter's current reachability model. Those semantic gaps
remain follow-up work; neither is exercised by these pinned fixtures. The broader deploy app
runner is still tracked under deploy issue 02.

### 2026-09-28 -- native asynchronous waiter semantics

Native `Atomics.waitAsync` now compares and registers on the process-wide FIFO waiter list under
one lock. `Atomics.notify` wakes its host request even if no store changes the cell; a store
without notify leaves it waiting until its actual timeout. The request parks without occupying a
realm lane, and only a real timeout uses a Tokio deadline. Cancelling the request or ending the
realm removes the waiter and releases its retained shared store. The pending wait is unreferenced,
matching Node's process-exit behavior. A cross-realm native probe checks notify-only wake,
store-only timeout and ordinary timeout; the focused native parity, realm-count and GC-stress
checks pass. Weak target cleanup remains separate work.
