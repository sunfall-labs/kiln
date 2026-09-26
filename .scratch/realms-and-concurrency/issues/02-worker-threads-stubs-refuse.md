# 02. `worker_threads` stubs refuse instead of doing nothing

Status: resolved

`crates/kiln-builtins/src/node/worker_threads.ts` defines a fallback `BroadcastChannel` whose
`postMessage` does nothing and a fallback `MessageChannel` whose ports are empty objects. A program
that uses them gets silently wrong answers, which is the failure `CLAUDE.md` calls the expensive one.
Until 06 lands, each should refuse naming the member, as `Worker` already does.

## Acceptance

- Using the fallback `BroadcastChannel`, `MessageChannel` or `MessagePort` throws a refusal naming
  the member, in both engines.
- A `conformance/native/*-refuses.ts` probe records the decision until 06 replaces it.

## Comments

2026-09-26. The fallback `MessageChannel`, `MessagePort` and `BroadcastChannel` apply only where the
realm has no web messaging globals, which is outside Node mode (`WEB_BOOTSTRAP_SOURCE` is loaded
only in Node mode, in both engines). In Node mode the real in-realm channels from
`web_events_streams.ts` are exported and were never the problem. Each stand-in now refuses by name.

Three more silent answers were in the same file, and are handled here:

- `receiveMessageOnPort` returned `undefined` for every port. It now takes the oldest message not
  yet dispatched, as Node does, started port or not, through a `receive` that `MessagePort` hands
  out from its private state (`conformance/regress/r18-worker-threads-receive-message.js`,
  expected output recorded from node 26.8.1). Landed together with issue 14's rewrite of
  `MessagePort`, whose private fields replaced the `_queue` this first read.
- `moveMessagePortToContext` returned its argument; it refuses.
- `markAsUncloneable` accepted a mark `structuredClone` cannot see, so a clone Node refuses would
  succeed; it refuses.

`markAsUntransferable` is honoured since issue 14 added transfer lists; `markAsUncloneable` still
refuses, because the structured clone keeps no uncloneable marks.

The record of the refusals is `conformance/native/node-worker-threads-refuses.cjs`.
