# Delete the bridge and the `_in_context` family

Status: ready-for-agent

Once nothing attaches through the bridge, delete it rather than leaving it as a fallback:

- `crates/kiln-runtime/src/http_transport.rs` (the duplex bridge, `HttpStreamAction`,
  `HttpHostOp`, `HttpCloseCapability`, `HttpResourceId`).
- `crates/kiln-runtime/src/hyper_http.rs` (both halves: `client_connection`,
  `event_connection`, the channel event vocabulary).
- In `lib.rs`: `RuntimeHttpConnection`, `start_http1_connection*`,
  `start_http1_client_connection`, `start_http_connection_close*`, `poll_http_connection*`,
  `poll_http_tasks_once`, `http_connection_started`, `complete_http_connection`,
  `close_http_connection`, `take_http_events`, `respond_http1`, `start/write/end_http1_response`,
  `send/write/end_http1_client_request`, `take_http1_client_events`, `flush_response_head`,
  `map_http1_client_event`, `start_host_op_in_context`.
- `http_body.rs`: the channel sink; only the direct sink remains.

`start_host_op` becomes the one entry: it starts on the realm directly when the caller is
already inside the executor and enters the executor otherwise. That is the last "am I in
context" question and it is answered once, by the runtime, instead of by a duplicated API.

The compile-fail doctests that pinned `HttpCloseCapability` go with the type they pinned; the
close authority they guarded (no raw close of an HTTP-owned resource) now rests on the engine
owning the transport outright, and `registered_http_connection_rejects_raw_hostops_close` still
pins the provider-side refusal. `scripts/check-http-close-contract.sh` keeps its doc-test step
because `kiln-runtime --doc` is still a valid, if now smaller, gate.
