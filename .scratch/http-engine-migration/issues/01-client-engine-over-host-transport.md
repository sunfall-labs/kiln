# Client engine: a Hyper client task over a host transport

Status: done

`hyper_http::client_connection` drives Hyper's HTTP/1 client over the `http_transport` bridge:
Hyper's reads become `TcpRead` host operations, its writes become `TcpWrite`s, and the realm
learns about the response through an unbounded channel it drains at a checkpoint.

Replace it with `HttpClientEngine` (`crates/kiln-runtime/src/http_client.rs`), shaped like the
server engine in `http_engine.rs`:

- `attach(resource, transport, dispatch)` spawns one task that performs the Hyper handshake over
  the transport and then waits for requests.
- `send_request(resource, id, method, target, headers, high_water_mark)` records the head in a
  request slot; `write_request` / `end_request` fill the body. The task takes the slot when it
  sends: a body that is complete by then declares its length (Node's `req.end(body)` rule), a
  streaming body is chunked, and a bodiless method with nothing written sends no body.
- Response head, chunk and end call the dispatch directly, inside the task's poll. `ResponseEnd`
  carries the Agent's reuse decision (`sender.ready()` after the body), as before.
- A connection error with no request in flight publishes only `Closed` (an idle keep-alive
  socket reset by the peer is not a request failure); with a request in flight, `Error{id}` first.
- `requests_in_flight()` counts requests whose end has not been dispatched, so a drive keeps
  waiting on them without a referenced host resource.

`ServerEvent` / `ServerEventKind` / `ServerDispatch` become `HttpEvent` / `HttpEventKind` /
`HttpDispatch` with the three response kinds, `status`, `reason` and `reusable` added, so one
dispatch implementation serves both roles (the archive's C event view already does).
`ServerCloseMode` folds into the existing `HttpCloseMode`.

## Comments

- Done in `a871b5c` ("Add the Hyper client engine over host transports and the queued
  adapter"). `crates/kiln-runtime/src/http_client.rs`; tests cover a complete body declaring
  its length, a streamed body chunking, sequential reuse with the `reusable` flag, an idle peer
  close publishing only `Closed`, and an abort failing the outstanding request before `Closed`.
