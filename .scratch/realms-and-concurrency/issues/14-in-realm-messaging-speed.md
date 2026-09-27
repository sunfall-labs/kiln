# 14. In-realm messaging at Node's speed, before `worker_threads` is built on it

Status: ready-for-agent

The concurrency baseline (`bench/concurrency/results-2026-09-26.md`, issue 13) measured kiln's
in-realm `MessageChannel` (`crates/semantics/kiln-builtins/src/web_events_streams.ts`) at 148 µs per
small-object ping-pong (node 25 µs, bun 9.3 µs), 8.4k one-way messages per second (node 126k,
bun 486k), 175 ms for a 1 MB object graph, and a "transferred" 16 MB `ArrayBuffer` costing about
what a copy costs (11 ms against 18 ms). `worker_threads` (06) will carry every message through
the same port and clone machinery, so this is fixed first.

Find where the time goes (the structured clone, `deliverLater` scheduling one `setImmediate` per
drain, `MessageEvent` construction, `EventTarget` dispatch) and fix it at the right layer: the
structured clone belongs in one shared implementation (ADR 0060), and transfer lists must detach the
sender's `ArrayBuffer` and hand over its backing store without copying (Node semantics; ADR 0070
primitive C's first user, issue 10).

## Acceptance

- Within 2x of node on the four messaging rows of the baseline, in both engines' Node mode.
- `postMessage(buf, [buf])` detaches `buf` in the sender, as node does, and costs the same for
  1 KB and 16 MB.

## Comments

**2026-09-26 — branch `alee/messaging-speed`: messaging 7-15x faster, transfer lists real, one
structured clone; acceptance met on ping-pong only, so this stays open.**

### What the profile showed (native, `inrealm.mjs`, component microbenchmarks + `perf`)

- **`hideSlots` was most of a message.** Event, MessageEvent and MessagePort redefined every
  `_` field with `Object.defineProperty` (~2.5 µs each natively) so `Object.keys` would be empty:
  `new Event` 31 µs, `new MessageEvent` 57 µs, against node's 0.2 and 3.4. Two of them per hop
  was ~120 of the 148 µs round trip.
- **The clone**: the native bootstrap's JavaScript clone walked ~12 `instanceof` checks and a
  dynamic-key write per property (14.5 µs for the small message, 77 ms for the 1 MB graph). The
  interpreter had a second clone in Rust that answered the *same* Map, Set and RegExp it was given.
- **Delivery**: one `setImmediate` per message with a closure; `arguments` materialised on every
  `postMessage`/`Event` construction (0.4 µs each natively).
- **Transfer was a copy.** The transfer list was ignored; `ArrayBuffer.prototype.transfer`
  (arraybuffer.ts, overriding the interpreter's real one) copied the bytes and set own
  `detached`/`byteLength` properties while every view still read the old bytes; natively
  `buf.detached` read `undefined` and `transfer()` refused.

### What changed

- **One structured clone** (`kiln-builtins/src/structured_clone.ts`, common bootstrap, ADR 0060
  kind 7). Deleted: the interpreter's `Native::StructuredClone` + `structured_clone_value` and the
  native bootstrap's JavaScript clone. Node's DataCloneError (a `DOMException`, `code` 25 — added
  the legacy codes to `DOMException`) and messages; transfer lists validated first and nothing
  detached until the whole value is copied; views rebuilt over moved buffers; `{ transfer }`
  options; `markAsUntransferable` honoured (and `isMarkedAsUntransferable`, a node export we
  lacked).
- **Three primitives**, captured and deleted by the bootstrap like `__kilnUrl*` (ADR 0065 has no
  table yet; each is documented where it is defined):
  - `__kilnArrayBufferDetach(buffer, length, preserve)` — representation. Interpreter: the old
    `ArrayBufferTransfer` arm, now a primitive (moves the `Vec`). Native: `kiln_typed_buffer`
    gained `detached`; the block moves to a new buffer (realloc only for a new length); views are
    emptied lazily in `kiln_c99_typed_array_of`/`data_view_of`, and the emitted element and
    `length` fast paths send a detached view to the generic path. `detached` is read like
    `byteLength`; constructing a view, `slice` and DataView access on a detached buffer throw.
    `transfer`/`transferToFixedLength` are TypeScript over it with node's errors.
  - `__kilnCloneData(value)` — *measured*: the plain-data subset (ordinary objects with the
    default prototype and only enumerable data properties, arrays without named properties,
    primitives) copied in one call, `undefined` otherwise so the TypeScript clone runs from the
    start; nothing in the subset can run program code, so the bail is unobservable. C in
    `runtime/spread.c` (shape and list mode), Rust in the interpreter. Graph clone 35 → ~9 ms.
  - `__kilnJobsPending()` — representation: what a microtask checkpoint would run now (bit 1
    ticks, bit 2 promise jobs). Native counts queued ticks in `kiln_gc_heap`; interpreter asks
    the realm (`has_queued_next_ticks`/`has_queued_microtasks`, added to `kiln-runtime`).
- **Messaging** (`web_events_streams.ts`): events and ports on private fields (Event state packed
  into four); `onmessage` is a listener in the position it was first set (node; `target` set,
  which it was not); `addEventListener("message")` starts a port; `postMessage(msg, transfer |
  { transfer })` returns `true` as node's does; a MessagePort in the list is transferred in-realm
  (new entangled port in `ev.ports`), one in the value but not the list is a DataCloneError.
  **Delivery is batched the way node does it**: one macrotask per port queue, and after each
  message, if a checkpoint has work, delivery continues from the back of it (behind ticks, then
  promise jobs, last hop always a tick so the next message's ticks still precede its promise
  jobs); two ports continuing at once fall back to a macrotask after 64 hops. `close()` delivers
  what was already queued first (node); BroadcastChannel gives each receiver its own copy.
- Interpreter bug found on the way: `setImmediate(fn, arg)` passed no arguments.

### Numbers

`bench/concurrency/run.sh --only messages`, 3 interleaved rounds each, same machine and hour;
before = origin/main `fa213915` kiln (load 9-12), after = this branch (two runs, load 10-14).

| in-realm `MessageChannel` | node | kiln before | node | kiln after (1) | node | kiln after (2) |
| --- | --- | --- | --- | --- | --- | --- |
| small object, ping-pong | 10.8 µs | 159 µs | 15.0 µs | 22.8 µs | 15.0 µs | 33.0 µs |
| small object, one-way | 374k/s | 7.3k/s | 207k/s | 108k/s | 354k/s | 75k/s |
| 1 MB graph, send + ack | 5.64 ms | 91.7 ms | 7.52 ms | 24.7 ms | 8.46 ms | 29.1 ms |
| 16 MB, copied | 17.3 ms | 8.87 ms | 16.2 ms | 11.0 ms | 14.7 ms | 11.7 ms |
| 16 MB, transferred | 0.035 ms | 5.56 ms | 0.038 ms | 0.090 ms | 0.035 ms | 0.10 ms |

Transfer cost is now independent of size: 1 KB 35-54 µs, 16 MB 37-49 µs per round trip (was
111 µs and 7.5 ms). Interpreter, wall time of self-contained fixed workloads (2,000 round
trips / 6,000 one-way / 3 graphs): 0.76 → 0.58-0.65 s, 1.20 → 0.67-0.73 s, 0.22 → 0.38 s (the old Rust
clone was faster on the graph because it did not copy Maps and Sets; node 0.09-0.12 s).

### Acceptance: not met on three rows

- **Ping-pong**: 1.5-2.2x node. Met in one run, borderline in the other.
- **One-way**: 1.9x and 4.7x in the two runs (node itself moved 207k → 354k/s). Per message
  kiln now spends ~57k user instructions against node's ~27k; the rest is general runtime cost —
  ~20% collector, generic property access on the list-mode objects the program itself builds.
- **Graph**: the clone is now within 1.3-1.5x of node's (9-11 ms vs 6.4-7), but the row also
  walks the copy, and `walkGraph` alone takes 9-11 ms natively against node's 0.12 ms: the
  records are list-mode (dictionary) objects, so every property read is a key comparison. That
  is object-model work, not messaging.
- **Transfer**: the row does one `touchBuffer` (4,096 typed-array reads) per round trip, 43-75 µs
  natively against node's 24 µs — most of the 90-100 µs. The transfer machinery itself is ~35 µs
  per round trip against node's 12.

### Known differences from node that remain

- A port's first delivery is a `setImmediate` at the first post, so an immediate scheduled
  *before* the post runs before the messages; node delivers in the poll phase, ahead of every
  immediate. Within and between batches node's order is matched (r17).
- An unhandled rejection from a message handler is reported after the batch, not before the
  next message.
- `Reflect.ownKeys(event)` is `[]`; node shows four symbol keys.

### Risks

- `emit_graph/ops/call_method.rs`'s `parallelMap` typed-array path reads `view->length` and the
  bytes directly; a `parallelMap` over a view whose buffer was transferred away would read past
  the detached placeholder. It needs `!view->buffer->detached` in its guard (left to the branch
  that owns that file).
- Method ids 532-534 are appended; a concurrent branch adding ids will collide on merge.
- Native hello world +0.3M instructions at start and +260 KB (+500 KB `--node`) binary; the
  interpreter's startup +50M instructions (a larger bootstrap to compile).

Probes: `conformance/regress/r17-message-port-delivery-and-transfer.mjs` (node-recorded),
`conformance/native/message-port-transfer.mjs` (both engines, also clean under GC stress).

### 2026-09-26, integrator: what remains open

Landed as `c3915a76`. Ping-pong is within 2x of node; one-way, the 1 MB graph and the transfer row
are not, and the profile says why:

- Cloned records come out as dictionary-mode objects, so reading the copy costs 9-11 ms natively
  against node's 0.12 ms. The fast clone (`__kilnCloneData`) must build shaped objects: a clone of
  an object with a shape should produce an object with the same shared shape (ADR 0071 makes the
  shapes of constant keys shared, so this is a transition walk, not a dictionary).
- Typed-array reads in the transfer row are 2-3x node's.
- Ordering: an immediate scheduled before the first `postMessage` runs before the messages (node
  delivers the messages first), and an unhandled rejection is reported after the batch rather
  than before the next message.
- `Reflect.ownKeys(event)` is `[]` where node shows four symbol keys.
- The three `__kiln*` bridge globals are interim primitives until architecture issue 08's
  declared primitive table exists; migrate them then.
- Found and fixed separately: the parallel typed-array fast path read detached views
  (`2c370546`, verified with the final combined run).
