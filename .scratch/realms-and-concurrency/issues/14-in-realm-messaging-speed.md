# 14. In-realm messaging at Node's speed, before `worker_threads` is built on it

Status: ready-for-agent

The concurrency baseline (`bench/concurrency/results-2026-09-26.md`, issue 13) measured kiln's
in-realm `MessageChannel` (`crates/kiln-builtins/src/web_events_streams.ts`) at 148 µs per
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
