# An agent-queued client request has no root, and its dispatch path looks broken anyway

Status: needs-triage

## What was found

Found while converting the node structs to the collector (ADR 0041). In `kiln_c99_node_http`,
when `kiln_realm_http_agent_acquire_active` answers `QUEUED`, the `kiln_node_client_request` is
registered with the Rust `HttpAgent` by request id only: it is not linked into any
`socket->client_requests` list, and the agent holds no C pointer to it. Its only root is the JS
`ClientRequest` object handed back to the caller. A program that discards that handle (fires a
request and forgets it) can now have the request collected while the agent still holds its id.

The `CONNECT` and `REUSE` decisions link the request into a socket's list and are rooted through
the socket, which the runtime's attached HTTP connection owner keeps alive.

## Why it is filed rather than fixed

The same path already looks wrong without a collector: on `DISPATCH`,
`kiln_c99_node_client_for_id` searches `socket->client_requests`, which a queued request was never
added to, so it returns NULL and the dispatch fails with `CONTROL_ERROR`. No probe in
`conformance/server` or `conformance/native` queues more than one client request per agent socket,
so nothing exercises this. Fixing the rooting without fixing the dispatch would preserve a request
that then cannot be dispatched.

## What to do

Give queued requests an owner on the C side (a per-agent or per-realm pending list that the unit
root function marks, or link them into the socket they will be dispatched to when the agent
decides), make `DISPATCH` find them there, and add a `conformance/server` probe that issues more
requests than `maxSockets` on one agent so both the queueing and the dispatch are observed.
