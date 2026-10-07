# Agent-queued client requests have C-side roots and a dispatch lookup

Status: resolved

## Original finding

The original report identified a real lifetime/lookup gap: a request accepted by the realm's
HTTP Agent as `QUEUED` was not linked into a socket's `client_requests` list, while event
dispatch searched that list. Dropping the JavaScript `ClientRequest` could also leave the
Agent's request ID without a live C request object.

## Current source

That gap is repaired in the current source. Before calling
`kiln_realm_http_agent_acquire_active`, `kiln_c99_node_http` registers the request object in
`realm->http_client_requests`. The realm GC visitor marks that value. Both connection-failure
retry and response-release `DISPATCH` find the C request by ID in this rooted table; dispatch
then attaches it to the selected socket. Completion, transport errors, connect failure,
cancellation, and socket teardown remove requests from the table. This is a bounded-in-flight
owner: completed and canceled requests are unregistered rather than retained for the realm's
lifetime.

Relevant implementation: `crates/native/kiln-backend-c/src/runtime/node.c` and the realm root
visit in `crates/native/kiln-backend-c/src/runtime/heap.c`.

## Remaining coverage limitation

This source repair has not been proven by a native end-to-end queued-request probe. The native
`node:http` facade does not currently expose `http.Agent` or a `maxSockets` option, and its
active acquire ABI does not pass an agent limit. The runtime Agent's default limit is
`usize::MAX`, so ordinary JavaScript probes cannot force the `QUEUED`/`DISPATCH` path. Existing
`kiln-runtime` Agent unit tests verify queue ordering and release decisions, but do not cover the
native C object's rooting, callback completion, or GC-stress behavior.

Do not count this as queued-client integration coverage. A future native `Agent`/`maxSockets`
surface should add a probe with `maxSockets: 1`, more than one request, and a dropped reference to
one queued handle; it should be run in ordinary and `KILN_GC_STRESS` native modes. That is a
separate API and coverage task, not an unresolved instance of the original missing-root or
missing-dispatch implementation.
