# Interpreter: queued dispatch onto the engines

Status: done

The interpreter (`crates/kiln-interpreter/src/lib.rs`) reaches HTTP through
`ActiveHttpRuntime`, and every method of that seam today returns `HttpStreamAction`s the
interpreter lowers to host operations and tracks as `PendingNodeRequest::Stream`.

ADR 0046 gives the interpreter the *queued* adapter: an `HttpDispatch` that converts each
`HttpEvent` into the `RuntimeHttpEvent` / `RuntimeHttpClientEvent` the interpreter already
consumes, pushes it on a per-resource queue in the realm state, and signals the runtime.

- `ActiveHttpRuntime` loses the action-returning methods (`connection_started`,
  `complete_connection`, `poll_connection*`, `start_connection_close*`); `start_connection`,
  `start_client_connection` and `close_connection` answer `bool`; `take_events` and
  `take_client_events` drain the queues.
- `RuntimeRuntime::next_turn` gains a third wake source: the queue signal. It yields
  `RuntimeTurn::Protocol`, which the loop answers by running its checkpoint. `next_referenced_turn`
  keeps waiting while an engine has a request in flight or a queue is non-empty, even when the
  host has nothing referenced (a taken transport is unreferenced by design).
- `checkpoint_runtime_http` drains queues; `start_runtime_http_stream_action`,
  `poll_runtime_http_writes` and the `runtime_http` branch of the `Stream` completion are deleted.
  `PendingNodeRequest::Stream` remains for raw `node:net` sockets.

## Comments

- Done (commit "Interpreter: attach HTTP connections through the queued adapter").
  `conformance/node --node` 35/35 and `conformance/server --node` 4/4 (2 skipped for missing
  packages), unchanged from the baseline; the node corpus runs in 11 ms against 87 ms before.
- One safety valve worth knowing: `checkpoint_runtime_http` discards queued events for any
  resource the realm no longer tracks. A queue nobody drains would otherwise keep
  `next_referenced_turn` waiting forever, because queued events count as live HTTP work.
