# ADR 0046: HTTP connections are Hyper tasks over host transports, and protocol events call the realm directly

## Status

Accepted. Refines ADR 0032 (which keeps its ownership split) and completes ADR 0040 step 4 for the
HTTP path. ADR 0041's collector is unchanged.

Implementation state (2026-09-02, later the same day): complete. Server connections are
`kiln-runtime/src/http_engine.rs`, client connections `http_client.rs`, and both attach the same
way: the host hands over its transport (`TokioHost` the socket, `ScriptedHost` a
`ScriptedTransport`) and one `HttpDispatch` receives every event. The native drive is one
future and its dispatch calls generated C directly; the interpreter uses the queued adapter
(`QueuedDispatch`, drained by its checkpoint, with `RuntimeTurn::Protocol` as the wake). The
bridge (`http_transport.rs`, `hyper_http.rs`, the runtime's bridged connection table), the
archive's `checkpoint_http` and `apply_http_actions`, and the `_in_context` family are deleted;
`start_host_op` answers "am I inside the executor" itself. The one departure from the text
below: the interpreter's checkpoint still turns the scheduler with `poll_http_tasks` between
drains, the open question the consequences mention.

## Context

After ADR 0041 and the checkpoint fix, a natively compiled `node:http` server serves about 56k
requests per second at sixteen connections against Node's 85k, at 17.8 µs per request. A profile
attributes the request like this: syscalls 25%, the C object model 10%, the allocator 8%, and
35% to runtime glue that exists only because of how bytes reach Hyper.

ADR 0032 chose to bridge Hyper to the realm through discrete `TcpRead` and `TcpWrite` host
operations so that a scripted host could replay a connection byte for byte. The price of that
choice is the whole glue column. One request today crosses: a spawned read task and a `JoinMap`
join, a re-sequencing `BTreeMap`, a duplex bridge that copies the bytes into Hyper's buffer, an
unbounded channel out of Hyper, a checkpoint loop over every open connection, a boxed event queued
as a microtask, and the mirror of all of that for the response, ending in a spawned write task.
Node's request is libuv into llhttp into the callback. Bun's is uWebSockets into the callback.

The replay property ADR 0032 wanted does not need discrete operations. Hyper is generic over
`AsyncRead + AsyncWrite`. A scripted transport that yields recorded bytes and records what is
written gives the same deterministic conformance run, on paused Tokio time, without a port, and
without any of the machinery between the socket and the parser.

## Decision

### The host provides a transport, not reads and writes

A HostOps provider that accepts or connects a TCP stream can hand its transport over:
`take_transport(resource) -> Option<HostTransport>`, where `HostTransport` is an owned
`AsyncRead + AsyncWrite` object. `TokioHost` returns the real socket. `ScriptedHost` returns a
scripted duplex that replays the recorded inbound bytes at the recorded points and captures the
outbound bytes for assertion. Raw `node:net` sockets keep the `StreamDriver` protocol; HTTP no
longer uses it.

### Hyper owns the connection; the realm owns the callbacks

An HTTP connection is one Hyper connection future over the host transport, spawned on the realm's
`LocalSet`. Its service calls the realm's HTTP adapter directly, on the realm thread, inside the
task's poll: request head, request body chunk, request end, response finished; for the client,
response head, chunk and end; for the listener, connection. The adapter is a trait with two
implementations, one that calls the language immediately (the native drive: a C function pointer
and its context) and one that queues events for a loop that consumes them later (the interpreter,
until it migrates). Dispatch is a Rust call; nothing is boxed, sent or re-sequenced.

Each callback ends with the realm's ordinary checkpoint: next-ticks, then microtasks, then the
unhandled-rejection check. That is the same contract Node gives an I/O callback, and it is what
makes calling from inside a task poll semantically identical to running the callback from a
queue. ADR 0032's sentence stands: a Hyper task becoming ready is not JavaScript ordering evidence.
What orders JavaScript is the checkpoint after every callback and the realm's own queues, not the
place the callback was entered from.

### A response is a slot the language fills

The service future does not wait on a channel. Each request owns a `ResponseSlot`: the adapter
writes the head and the body driver into it when `res.writeHead`/`res.end` run, and the service
future, awaiting the slot, resumes and hands Hyper the response. A body that streams keeps the
existing `ResponseBody` shape behind the slot. A response produced inside the request callback
(the common case) costs one slot write and no wake.

### The drive is one future

With connections dispatching for themselves, the drive no longer polls connections or drains
events. It is one `block_on` for the life of the program: run queued realm work, run due timers,
await whichever of a timer or a host completion comes first, and stop when no referenced work
remains. Everything the language re-enters from a callback is already in context, so the
`_in_context` variants and the `in_context` forks are deleted, which is ADR 0040 step 4.

### The listener accepts continuously

The listener is a task that accepts in a loop and dispatches `connection` directly, spawning the
connection future before it dispatches. One accept in flight per realm turn was a round trip
through the realm per connection; it is gone.

## Consequences

- The request path becomes syscall, Hyper parse, one call into the language, Hyper encode,
  syscall. The glue column of the profile is removed rather than shrunk.
- `http_transport.rs`, `http_body.rs`'s duplex state, the HTTP branch of `StreamDriver`, the
  `http_requests` map, `checkpoint_http`, `apply_http_actions`, `take_http_events` and the
  `_in_context` family are deleted. The runtime gets smaller.
- Scripted HTTP tests move from "expect this `TcpRead`, answer with these bytes" to "these bytes
  arrive, these bytes must leave". That is a large mechanical rewrite of the runtime's tests and
  the reason this is a decision rather than a tuning pass.
- The interpreter keeps working through the queued adapter and can adopt the direct one when its
  loop is ready. Its checkpoint's dependence on reactor turns (noted in the runtime) is
  unaffected by this change and remains an open question.
- Concurrency order across connections is Hyper's task order, as it already was in effect.
  Single-connection scripted runs are fully deterministic.

## Alternatives rejected

**Keep the bridge and optimise its pieces.** Each piece is small; there are eleven of them per
request, and the profile says their sum is the problem. Pooling buffers or caching the connection
lookup leaves the shape that produces the cost.

**Replace Hyper with a hand-written HTTP/1 parser called from the read task.** That is the path
ADR 0032 correctly closed: Kiln would own framing, keep-alive, pipelining and smuggling defences
for no gain, since Hyper's parser is not where the time goes.

**Call the language from the I/O task but keep the discrete host operations.** Halves the glue
and keeps the replay machinery. The scripted transport is simpler than the machinery it replaces,
so there is no reason to keep both.
