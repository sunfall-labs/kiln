# 21. Cross-realm messaging at node's speed

Status: resolved (four paired native-release rows meet their Node ceilings; program gate passed)

Found by issue 06: `worker_threads` messages between realms cost 108 µs per small-object ping-pong
(node 20.4 µs), 64.5k one-way messages per second (node 499k), 78 ms for a 1 MB object graph (node
5.5 ms) and 1.8 ms to transfer a 16 MB buffer (node 0.04 ms). Each message is encoded and decoded in
TypeScript into tokens and crosses a Tokio host-op round trip (`AsyncHostOp::PortWait`).

Move the encoding to one shared Rust engine behind the provider (ADR 0060 kind 2), batch deliveries
per turn, hand a receiving realm the decoded graph directly where the value is plain data, and make
a transferred buffer a pointer handoff with no per-message host round trip. Where a message is
immutable data, ADR 0070's shared immutable region (issue 08) removes the copy altogether.

## Acceptance

- Within 2x of node on the four cross-realm messaging rows of `bench/concurrency`; a 16 MB transfer
  independent of size and within 5x of node.

## Comments

### 2026-09-29 -- plain-encoder provenance removes redundant receiver preflight

The shared Rust plain-graph encoder already emits only the native decoder's supported wire
tags. A message now carries a private provenance bit set only by `Message::from_plain`, which
runs that encoder; ordinary and transferred messages leave it clear. The native receiver
skips its recursive preflight for a certified wire, while its construction walk still checks
tags, lengths and reference bounds. This removes a second complete traversal of the large
graph without weakening the fallback for other token producers. The provider take ABI is 63.

Eight alternating graph-only old/new pairs, with matching checksums, moved the median from
11.042 to 10.711 ms. Four further interleaved Node/old/final rounds on the exact post-review
binary matched every checksum and measured:

| Row | Node | old kiln | final kiln | final / Node |
| --- | ---: | ---: | ---: | ---: |
| Small ping-pong | 19.334 µs | 32.116 µs | 32.304 µs | 1.67× |
| Small one-way | 678k/s | 375k/s | 378k/s | 1.79× slower |
| 1 MB graph | 5.666 ms | 10.500 ms | 10.244 ms | 1.81× |
| 16 MB transfer | 0.03983 ms | 0.07909 ms | 0.08026 ms | 2.01×, within the separate 5× limit |

The transfer path is unchanged; the prior size-independence evidence below still applies.
Raw rows are in `target/messages-plain-graph-paired.json` and
`target/messages-plain-final-paired.json`. The graph improvement is modest, so the ratio is a
same-host acceptance measurement rather than a guarantee on every machine.

Focused runtime and provider worker tests passed 16 and 2 cases; the contract passed 12 unit
and 5 documentation cases; backend C passed 257 unit cases and 36 emitted-C cases. Two worker
graph probes matched interpreter output and status with native GC stress. The same two probes
matched alone and at 1, 2 and 8 realms with every-allocation GC stress. The full program gate
remains due.

### 2026-09-29 -- realm cache registration and plain-graph encode costs

Profiling the native release message workload found that repeated registration of the same
process-wide cache site in each realm filled a realm's site list with duplicates. Reaching
capacity then sorted and deduplicated the whole list during message traffic. Realm-local site
and private-site lists now stay sorted and unique on insertion. Six alternating old/new runs
lowered the small ping-pong median from 41.15 to 32.34 microseconds, with matching checksums.

The native plain-graph adapter also converted every packed array element and shaped-object slot
to a boundary value and straight back before passing it to the shared Rust encoder. Its callbacks
now read those packed values directly; the existing guarded fallback still handles unsupported
graphs. Six paired runs lowered the 1 MB graph median from 11.31 to 11.00 milliseconds. Changing
only the encoder's per-message identity table to the workspace's `AHashMap` lowered the graph
median from 10.91 to 10.51 milliseconds in six more paired runs. An `FxHashMap` trial bought less
than half a percent on six paired runs and was rejected.

Four interleaved full-budget Node/native-release runs after these changes matched every checksum.
Small ping-pong was 31.738 versus Node's 18.219 microseconds (1.74x), one-way delivery was 329k
versus Node's 602k messages/second (1.83x slower), and 16 MB transfer was 0.07837 versus 0.03633
milliseconds (2.16x, within its 5x limit). The 1 MB graph was 10.453 versus 5.1015 milliseconds
(2.049x), still just outside the 2x target. This issue remains open. Raw paired and final rows
are in `target/messages-*-paired.json` and `target/messages-issue21-current-full.json`.

Backend C tests passed 255 cases with one ignored, and 36 emitted-C snapshots passed. The Rust
worker-codec tests passed 15 cases. Four focused native worker probes matched under forced GC.
Two plain-graph/sparse-array probes matched at 1, 2 and 8 realms under forced GC; three
cache/private-site probes then matched at the same realm counts under forced GC after the final
release rebuild. No divergence was observed in those focused checks.

A bounded root-length reservation for the Rust identity table and token vector was also tried.
Eight alternating full-budget graph-only pairs moved the median from 10.536 to 10.468
milliseconds (0.6%); all checksums matched. The reservation was reverted because it does not
materially close the gap. The remaining profile points to graph encoding and receive token
conversion; changing their representation needs an end-to-end design and measurement, not
another root-size heuristic. The paired rows are in `target/messages-reserve-paired.json`.

The native field callback was then made to return a full child description with its handle,
so the Rust graph walker would skip a second callback for each non-hole child. Touched crates
compiled; backend C, interpreter, provider (excluding the sandbox's TCP-listener fixture) and
worker-codec checks passed. Eight alternating native-release pairs with matching checksums
found a graph regression instead: 10.576 to 11.331 milliseconds (7.1% slower), while small
ping-pong was unchanged. The larger per-child FFI record and eager description cost more than
the callback saved. The ABI and source change were reverted. Raw rows are in
`target/messages-predecoded-paired.json`.

### 2026-09-29 -- acceptance recheck after the actor and graph work

The three-round non-server comparison in issue 13 checked every messaging answer but placed
kiln just beyond this issue's 2× Node ceiling on small ping-pong (41.5 versus 18.5 µs) and a
1 MB graph (11.4 versus 5.18 ms). To separate the new suite's larger sample budget from the
original acceptance workload, four fresh interleaved runs used the same
`messages.mjs 120 3000 all` invocation as the accepted 2026-09-27 measurement. All checksums
matched. Their medians were:

| Row | Node | kiln | Ratio | Current verdict |
| --- | ---: | ---: | ---: | --- |
| Small ping-pong | 19.30 µs | 41.05 µs | 2.13× | open |
| Small one-way | 675.2k/s | 374.1k/s | 1.80× slower | met |
| 1 MB graph | 5.540 ms | 11.285 ms | 2.04× | open |
| 16 MB transfer | 0.03773 ms | 0.08112 ms | 2.15× | within the 5× transfer limit; above 2× |

Raw rows are in `target/messages-issue21-recheck.json`. The prior acceptance result was
near the threshold, and current evidence no longer supports a resolved status. Re-profile
delivery and graph construction against this exact workload, preserve the shared Rust codec
and guarded TypeScript fallback, then remeasure before closing this issue.

One hot-path hypothesis was tested and reverted: avoid cloning `WorkerContext` in
`workers::take` for non-actor ports, consulting the worker key only if the endpoint has an
actor-reader restriction. The existing actor-reader unit cases passed, but six alternating
old/new native-release runs showed no win: ping-pong 41.63→41.82 µs, 1 MB graph
11.31→11.38 ms, and one-way throughput 372→367k/s. The source was restored exactly;
the paired rows are in `target/messages-take-guard-paired.json`.

### 2026-09-27 -- guarded native plain-data representation path (partial)

The native sender now walks a safe object/array graph directly into the existing message wire,
avoiding an intermediate cloned graph. The receiving native realm preflights that wire and builds
plain objects, arrays and buffers directly in its heap, with shared references and cycles rooted
during construction. Getters, proxies, special attributes, ports and unsupported tags fall back
before posting or adopting a transferred buffer to the existing TypeScript structured clone.
The TypeScript implementation remains the semantic owner under ADR 0060; this C path is a guarded
representation specialization, not the requested shared Rust engine.

The first version added a `__kilnEncodeMessageData` global and the native corpus caught it:
`global-surface` and `global-callable-names` diverged in programs that do not run the Node
bootstrap, so its capture/delete never ran. The path now uses mode 1 of the existing captured
`__kilnCloneData` primitive. The interpreter answers `undefined` for that mode and follows the
TypeScript encoder. This avoids a new global and method ID; the full native corpus rerun is the
verification gate for the surface repair: 312 matched, 1 refused, 0 diverged.

Focused native-release measurements on this branch, same `120 3000 all` invocation as the Node
baseline, checksums matched:

| Row | Node | Before | This branch | Acceptance |
| --- | ---: | ---: | ---: | --- |
| small ping-pong | 20.25 µs | 73.9 µs | 50.09 µs | ≤ 40.5 µs |
| small one-way | 626k/s | 150.8k/s | 200.7k/s | ≥ 313k/s |
| 1 MB graph | 5.55 ms | 32.8 ms | 11.94 ms | ≤ 11.1 ms |
| 16 MB transfer | 0.039 ms | 0.973 ms | 0.861 ms | ≤ 0.195 ms |

The regular transfer row samples 4,096 bytes on each side. A separate no-touch round trip
confirmed the backing block handoff is size-independent: kiln 0.118/0.210/0.188 ms for 1/16/64 MB,
Node 0.038/0.022/0.021 ms. Its 16 MB cost is still about 9× Node.

The remaining architecture and speed work:

1. Put the clone graph/wire algorithm in a shared Rust message codec behind the provider.
   Give it graph descriptors from each heap adapter, including identity, own keys, holes and
   transferable handles; let it own reference numbering, traversal order, validation and the
   wire. Keep the TypeScript structured clone for guarded fallback and user-observable getters.
   Delete the C walk when the shared path supersedes it, per ADR 0060.
2. Add a provider `take_batch` that drains a bounded number of messages under one queue lock.
   The receiving realm still dispatches one at a time and checks tick/microtask work between
   callbacks. Batch only the transport, preserving message and close order.
3. Use the already moved `Token::Moved` backing block through a direct provider handoff to the
   target realm and wake its executor without a fresh Tokio `PortWait` per message. Keep transfer
   validation and detach atomic before posting; verify no byte-length slope from 1 to 64 MB.
4. Re-measure all four official rows and the no-touch transfer row on a quiet machine. None of
   the four official targets has yet been met, so this issue remains in progress.

The focused parity probe exercises cycles, sharing, holes, Unicode strings, an own `__proto__` data
property, getter fallback and transfer against Node, interpreter and native. A separate Proxy
probe records an existing gap: Node throws `DataCloneError`, the interpreter accepts it, and
native TypeScript fallback refuses `Array.prototype.push` on receiver tag 11. The C encoder
does reject the Proxy before touching it; resolving that gap belongs to clone parity.
The representative `scripts/native-realms.sh` run of the plain-graph probe matched the lone
program at 1, 2 and 16 realms with no skips or divergences.
The receiving array materializer now fills its uninitialized capacity with `HOLE` before
publishing `count`, so a collection during recursive decode never traces stale element bits.
The probe includes 64 nested objects to force collections during that fill; it matches Node,
interpreter and native with `KILN_GC_STRESS=1`. The ordinary native corpus after this fix again
reported 312 matched, 1 refused, 0 diverged.

### 2026-09-27 -- one shared Rust plain-graph encoder (partial)

The guarded no-transfer sender now uses `kiln_runtime::workers::encode_plain` in both engines.
Each heap adapter describes own plain-data fields without invoking user code; Rust owns DFS,
identity/reference numbering, keys, holes and token order. The native provider posts the resulting
`Vec<Token>` directly, without materializing a JavaScript token array or converting it back into
provider tokens. The old C message-wire walker and its TypeScript mode-1 call are gone. A guarded
bailout still runs the full TypeScript structured clone before posting. The native provider ABI is
50 with 141 exports (`kiln_worker_post_plain`). No transfer is committed in this new path.

Sparse arrays now decline the guarded clone in both engines. The TypeScript clone and its message
wire encoder copy only own array indices; previously an inherited numeric getter could be read by
`slice` or by `i in value` and copied as an own element. The new native corpus probe checks an
inherited numeric data index both with and without a transfer list. A separate scratch probe
checks an inherited getter: Node and the interpreter both leave the hole absent and read the
getter zero times. Native currently refuses defining an accessor on `Array.prototype` (receiver
tag 8), so that getter probe is not a native-corpus gate. The existing plain graph and transfer
probes continue to check cycles, aliasing, getters, Unicode, closed endpoints and transfer order.

Validation on the isolated worktree: backend C unit 217 and emitted-C 36, interpreter 200,
runtime 169, builtins 98, provider FFI 38, provider contract 11 + 5, all passing. Ordinary
`scripts/native.sh`: 317 matched, 1 refused (`shadow-realm`), 0 diverged. Forced-GC plain-graph
and sparse probes: 2 matched, 0 refused, 0 diverged. Three interleaved quiet rounds of the
official `messages.mjs 120 3000 all` rows produced matching checksums:

| Row | Node median | Shared Rust codec median | Target |
| --- | ---: | ---: | ---: |
| Small ping-pong | 19.80 µs | 48.84 µs | ≤ 39.61 µs |
| Small one-way | 668.9k/s | 195.3k/s | ≥ 334.5k/s |
| 1 MB graph | 5.559 ms | 10.727 ms | ≤ 11.118 ms |
| 16 MB transfer | 0.0390 ms | 0.881 ms | ≤ 0.195 ms |

The graph row now meets the 2× target. The other three do not, so issue 21 remains in progress.
A provider-owned batch lease of 16 raw messages was built and passed parity but regressed the
official rows (ping 49.88→54.50 µs, one-way 201.4k→174.8k/s, graph 12.27→13.60 ms,
transfer 0.865→1.190 ms) and was reverted. A narrow guarded transfer encoder saved only 1.8%
in the transfer row and was also reverted. The remaining work is transport delivery and the
receiving/transfer path: batch at a turn boundary only if it reduces lock or wake cost without
changing callback order, and hand a moved backing block to the receiving realm with one owner.
Profile the no-touch transfer and the benchmark's 4,096 typed-byte reads separately, then
re-measure all four acceptance rows and 1/16/64 MB no-touch transfers.

### 2026-09-27 -- moved-buffer collection debt (partial)

The 16 MB backing store was already handed between realms by pointer. The receiver nonetheless
charged its full size as fresh heap allocation at each adoption. With the 1 MB graph row run
first, one realm retains about 31,976 graph blocks (3.1 MB), so that 16 MB charge triggered a
full collection and graph scan on every transfer. An isolated transfer row took about 0.27 ms;
the same row after the graph took about 0.95 ms. The copy row did not cause the slowdown.

The collector now tracks the current bytes owned by moved buffers in each realm and uses the
last collection's total as a baseline. Sending a block lowers the sender's total, adoption
raises the receiver's, and finalization lowers it exactly once. Repeated trips of one block
create no new collection debt after the first scan; distinct incoming blocks still increase the
total and cause a collection. In-realm `ArrayBuffer.transfer` carries the accounting through
resize and shrink. The dormant typed-buffer heap-splice path also moves accounting and backing
store ownership; its direct C fixture checks both tenured splicing and nursery promotion.

Before the scheduler branch landed, four interleaved quiet runs of `graph-1mb,ab16-transfer`
with matching checksums gave a transfer median of 0.8697 ms before this change and 0.0878 ms
after it (9.9× faster). This is paired evidence for the accounting change, not the final
acceptance measurement. The new pressure probe sends one 1 MB block back and forth 12 times,
resizes it in the receiving realm, then sends 12 separate blocks to be dropped. It prints
`12 12 150 1048576` in Node, the interpreter, native, and native with `KILN_GC_STRESS=1`.
Backend C unit fixtures passed 217/217, the focused splice fixture passed, and emitted-C
snapshots passed 36/36 after re-recording the changed ABI surface.

On the rebased scheduler branch, the ordinary native corpus reports 318 matched, 1 refused
(`shadow-realm`), 0 diverged. Three interleaved quiet Node/native rounds of the full official
message workload produced matching checksums:

| Row | Node median | kiln median | Target | Verdict |
| --- | ---: | ---: | ---: | --- |
| Small ping-pong | 20.04 µs | 51.51 µs | ≤ 40.08 µs | open |
| Small one-way | 670.1k/s | 191.3k/s | ≥ 335.1k/s | open |
| 1 MB graph | 5.822 ms | 11.214 ms | ≤ 11.643 ms | met |
| 16 MB transfer | 0.03909 ms | 0.09122 ms | ≤ 0.19547 ms | met |

The separate 1/16/64 MB no-touch transfer probe, three rounds with matching checksums,
measured 0.0772/0.0791/0.0838 ms in kiln and 0.0393/0.0212/0.0203 ms in Node. It has
no material byte-length slope and stays within 5× Node at each size. Ping-pong and one-way
throughput remain open, so issue 21 stays in progress.

### 2026-09-27 -- direct completion delivery and bounded native key reuse

The receiving realm now registers its pending port wait directly with the endpoint's completion
signal. Posting or closing wakes that realm without creating a Tokio task for each wait. The native
host completion invokes a GC-rooted callback in the I/O turn, so message delivery no longer needs
an extra promise reaction and queued I/O turn. The interpreter retains its promise path. The
existing bounded `deliverSome` loop drains messages in one turn and checks tick/microtask work
after each callback. A provider lease that fetched 16 raw messages under one lock had regressed
every messaging row in the earlier experiment, so transport still takes each message separately.

For a sole Node-style message listener on a plain remote port, delivery calls its data listener
directly. It checks the live listener table, so removal, replacement, once listeners, EventTarget
listeners, transferred ports and message errors still use normal event dispatch. The native
receiver also unwraps the direct plain-data result without allocating an intermediate two-element
array. In the native adapter for the shared Rust encoder, an eight-entry per-encode cache reuses
owned string units for repeated object keys once the graph has at least 16 compound nodes. The
cache compares contents because C can reuse the descriptor scratch storage between callbacks;
the Rust codec still owns traversal, identity, references and token order.

Four interleaved quiet rounds of `messages.mjs 120 3000 all` on this isolated branch, with all
five checksums matching Node, gave these medians:

| Row | Node | kiln | Ratio | Target |
| --- | ---: | ---: | ---: | --- |
| Small ping-pong | 19.146 µs | 37.710 µs | 1.97× | ≤ 2× |
| Small one-way | 687.5k/s | 370.3k/s | 1.86× slower | ≤ 2× slower |
| 1 MB graph | 5.542 ms | 10.056 ms | 1.81× | ≤ 2× |
| 16 MB transfer | 0.03749 ms | 0.07395 ms | 1.97× | ≤ 2× |

The paired old/new binary comparison isolates the key cache: graph median 10.877→10.056 ms,
while ping-pong, one-way and transfer stayed within noise. The moved block path is unchanged from
the earlier 1/16/64 MB no-touch sweep and remains independent of size. The receiver builds the
plain graph directly in its heap from the shared Rust token stream; a moved buffer's backing store
is adopted by pointer.

Final validation on this branch: provider FFI 39 unit tests, runtime 182, builtins 98, backend C
218 plus 36 emitted-C snapshots all passed; the full ordinary native corpus matched 319 probes,
with one existing `shadow-realm` refusal and zero divergences. The direct-listener and cyclic
64-node plain-graph probes matched under forced GC (2/2). Those two probes also matched their
lone-program output at 1, 2 and 16 concurrent realms (2 matched, none skipped or diverged).
The four official speed rows and size-independent transfer target now meet acceptance.
