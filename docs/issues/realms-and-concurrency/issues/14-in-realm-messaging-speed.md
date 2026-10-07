# 14. In-realm messaging at Node's speed, before `worker_threads` is built on it

Status: resolved

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

**2026-09-26 — branch `work/messaging-speed`: messaging 7-15x faster, transfer lists real, one
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

**2026-09-27 — branch `work/messaging-finish` (`1e673f1f`, `9984c599`, `198cae40`): the four
open items, each at its layer. Stays open: one-way is ~3x node and the transfer row ~2.2x.**

### (a) Cloned records

The diagnosis above did not survive measurement. The benchmark's graph records are shaped, and
`__kilnCloneData` already gave each copy its source's shared shape (`memcpy` of the slots plus
`copy->shape = shape`): a property read costs 4-5 ns on the original and on the copy alike. The
9-11 ms "walk" was the rest of `walkGraph` (a `for...of` over an array is ~500 ns an iteration
natively, a separate cost), and with the last branch's clone the 1 MB graph row is at node's time
(table below; 1.04G instructions against node's 1.81G on a fixed 20-round workload).

What did come out dictionary-mode was the copy of a *list-mode* source -- an object that lost a
property, or had an attribute redefined. `runtime/spread.c` now gives it the transition walk from
the empty shape over the source's keys in order (ADR 0071: the shape an object literal with those
keys has); an index-like key or the fast-property limit leaves it in list mode. Reading such a
copy: 38 -> 17 ns a record. The TypeScript clone builds its copies with `{ ...value }` onto a
fresh shaped object and was not changed. Probe: `conformance/native/structured-clone-dictionary-source.js`.

### (b) Typed-array reads

Two things, both in `emit_graph/ops/properties.rs`:

- **A wrong-answer bug.** The element read, element write and `length` read remembered the
  first array a site saw (`site.has_cached_method`) and took every later buffer-tagged value for
  an array. A typed view with a byte offset was then read as a `kiln_array`: `length` answered
  the byte offset (8 for a 4-byte view), a read answered the buffer pointer's bits, and a write
  landed in the view's buffer header. The payload's kind is asked on every access now. Probe:
  `conformance/native/typed-array-after-array-site.js`.
- **The load.** A read that passed the emitted bounds check called `kiln_c99_typed_get` in the
  prelude object, which checked the bounds again and assembled the element byte by byte. It is a
  header inline now (`kiln_typed_load`, `prelude_types.h`): 96 -> 55 instructions per element in
  a dense loop. What is left per element is the call to `kiln_c99_gc_kind_of_ptr`, which lives in
  the prelude object because the collector's block header is not in the module header.

Found on the way, not fixed: **`BigInt64Array`/`BigUint64Array` elements read as `NaN` natively**
(`typeof new BigInt64Array([5n])[0]` is `"number"` in every profile; `kiln_c99_typed_get` has no
BigInt case). The fast paths now leave those kinds to the generic path, which is where the fix
belongs.

### (c) Delivery order

- **Immediates.** An immediate was a zero-delay timeout and a delivery an immediate. The realm
  (`kiln-runtime`, `Realm`) now has node's three macrotask lanes, taken in the order of one libuv
  turn: the timers due when the turn begins (a timer armed by one of them waits for the next
  turn, even with no delay), the I/O callbacks queued when the poll phase begins, the immediates
  queued when the check phase begins. `setImmediate` is the check lane in both engines
  (`kiln_realm_set_immediate`; provider ABI 43), and a port's delivery is the I/O lane through a
  new primitive, `__kilnQueueIo` (`kiln_realm_queue_io`, method 535), captured and deleted by
  `web_events_streams.ts` like `__kilnJobsPending`. The lanes cost nothing measurable: delivering
  through `setImmediate` instead gives the same instruction count to the unit.
- **Unhandled rejections.** `__kilnJobsPending` answers 4 while a rejection nothing handled is
  waiting, and a delivery ends its batch there, so the loop reports it before the next message.
  With the interpreter, `msg 1, tick 1, msg 2, tick 2`, then the report and exit 1 -- node's
  stdout and status. **The native backend never reports an unhandled rejection at all** (exit 0,
  no message; there is no `handled` bit on a native promise), so there the bit is always 0. That
  is its own work item, not messaging's, and it is also what keeps this half out of the regress
  corpus: neither engine implements `process.on("unhandledRejection")`, and a recorded crash is
  not something `expected_stdout` can hold.
- Recorded against node 26.8.1 in `conformance/regress/r19-message-port-poll-phase-and-event-slots`
  (both engines print it). One difference kept out of the record: node clamps a `setTimeout`
  delay below 1 ms to 1 ms, so its `setTimeout(f, 0)` from a timer callback can run after two
  turns of immediates where kiln's runs on the next turn.

### (d) Event symbol keys

Node 26 keeps `Symbol(type)`, `Symbol(kTarget)`, `Symbol(kIsBeingDispatched)` and
`Symbol(kInPassiveListener)` as own enumerable data properties of every event (a CustomEvent adds
`Symbol(detail)`), and they are its state: writing `Symbol(type)` changes `event.type`, and
`isEvent` is "has a string there". They are that here now, with node's `ERR_INVALID_THIS` for a
getter on a non-event. Not matched: node's `util.inspect.custom` on `Event.prototype`.

The cost is real. A symbol-keyed define or store goes through the native object model's generic
path (`kiln_c99_key_hash` and interning on every access, ~1.9k instructions each), and an event
now makes four defines and three dispatch stores: **+24% instructions per ping-pong round trip
and +31% per one-way message.** The fix belongs to the object model, not to events: a
symbol-keyed access whose key is a constant could take the shape path a named one takes. That
code (`boxed.c`, `read.c`, `write.c`) is the concurrent-realms branch's today, so it is left.

### Numbers

Instructions (`perf stat -e instructions:u`), fixed workloads with the rows' own messages:
20,000 round trips, 50,000 one-way, 20 graph round trips, 2,000 16 MB transfers (the last
includes filling the buffer once, ~6G of it). `base` is origin/main `0da8a1c4`.

| workload | base | this branch | node 26.8.1 |
| --- | --- | --- | --- |
| ping-pong ×20k | 2.64G | 3.27G | 1.49G |
| one-way ×50k | 2.50G | 3.29G | 1.35G |
| graph ×20 | 1.04G | 1.05G | 1.81G |
| transfer ×2k | 8.58G | 7.80G | 1.28G |

`bench/concurrency/js/inrealm.mjs`, 3 interleaved rounds, same binaries, load 13-20 on 16
threads (a later set at load 20-25 was 2-5x slower in every column and is not used):

| in-realm `MessageChannel` | node | base | this branch |
| --- | --- | --- | --- |
| small object, ping-pong | 9.9-10.9 µs | 16.0-16.3 µs | 18.4-18.5 µs |
| small object, one-way | 495-504k/s | 166-179k/s | 145-152k/s |
| 1 MB graph, send + ack | 5.4-5.5 ms | 5.5-6.2 ms | 5.6-6.0 ms |
| 16 MB, copied | 13.5-16.4 ms | 5.4-6.2 ms | 4.8-6.1 ms |
| 16 MB, transferred | 0.030-0.031 ms | 0.077-0.079 ms | 0.065-0.068 ms |

### Acceptance

- Ping-pong 1.7-1.9x node: met, with less margin than before (d).
- One-way ~3x: not met. The per-message cost is general runtime cost, now plus (d)'s symbol
  stores; both are object-model work.
- Graph: met (at node's time).
- Transfer ~2.2x: not met. Size-independent (1 KB and 16 MB cost the same per round trip), and
  the row is `touchBuffer`'s 4,096 strided reads plus the messaging; the remaining per-read cost
  is the out-of-line kind check.

**2026-09-27 — resolved on `origin/main` at `f3b372a6`; no further runtime change needed.**

The later symbol-keyed property work (issue 17) and cross-realm message delivery work (issue 21)
changed the paths this in-realm benchmark uses. Measured a release CLI built with `mbx` and its
`--node --profile native-release` binary from an isolated worktree. Three alternating Node 26.8.1
and kiln runs of `bench/concurrency/js/inrealm.mjs 120 3000 all` had load averages 5.5-5.8.
Medians below use the same workload in both runtimes; all five checksums matched in every round.

| in-realm `MessageChannel` | Node | kiln | kiln / Node time | target |
| --- | ---: | ---: | ---: | --- |
| small object, ping-pong | 9.366 µs | 10.559 µs | 1.13x | at most 2x |
| small object, one-way | 568.2k/s | 318.6k/s | 1.78x | at most 2x |
| 1 MB graph, send + ack | 5.226 ms | 4.230 ms | 0.81x | at most 2x |
| 16 MB, copied | 18.088 ms | 6.458 ms | 0.36x | context |
| 16 MB, transferred | 0.02714 ms | 0.04238 ms | 1.56x | at most 2x |

A separate in-realm no-touch round trip transferred the same buffer both ways, checking that the
sender was detached and that the returned length and first byte survived each trip. With 100 warmup
and 1,000 timed trips per size, three-run medians were 16.703 µs for 1 KB and 16.399 µs for 16 MB
in kiln (0.98x size ratio); Node measured 9.659 and 9.656 µs. The transfer cost has no byte-length
slope. `conformance/native/message-port-transfer.mjs` printed identical output in Node, the kiln
interpreter, and kiln native. The five benchmark checksums also matched Node in the interpreter
with a reduced workload (`0 100 all`); its virtual clock makes its timing fields unsuitable for
this speed comparison.

### 2026-09-29 -- quiet structured-clone startup audit

The exact `fa213915`/`c3915a76` pair confirms the historical interpreter typed-array
loop cost (1,110 -> 1,389 ms), native hello size growth (+238 KiB, +469 KiB with
Node), and interpreter hello startup growth (76.6 -> 78.6 ms; Node 92.6 -> 99.4 ms).
Three-round paired current production measurements show the selected interpreter loop
is now 731 ms and hello startup is faster than the pre-change endpoint. The historical
loop regression does not persist. Current Node native hello remains about 0.54 ms slower
and larger, across the full intervening history; that belongs to issue 08's bootstrap
work, not an attributed clone regression. These are external timings and sizes, not
hardware instruction/cycle counts. See `../performance-audit.md` and its durable
structured-clone evidence for all 66 checked cells and exact sources.
