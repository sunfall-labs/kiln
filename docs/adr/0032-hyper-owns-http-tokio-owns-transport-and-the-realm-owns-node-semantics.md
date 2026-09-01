# ADR 0032: Hyper owns HTTP, Tokio owns transport, and the realm owns Node semantics

## Status

Accepted.

This refines ADRs 0028, 0029, and 0031. It replaces Kiln's incremental HTTP/1 parser and encoder
once the migration reaches each connection path; it does not replace the realm scheduler,
HostOps, or Node stream state machines.

## Context

Kiln now runs `node:http` servers and `http.get` over Tokio-owned TCP resources. The first vertical
slice deliberately used a small protocol implementation in `kiln-runtime::node_http`. That proved
the module, generated-C, opaque-provider, completion, and shutdown boundaries, but continuing to
grow it would make Kiln responsible for HTTP framing, connection reuse, informational responses,
upgrades, trailers, chunking, and request-smuggling defenses.

Hyper is the mature Rust implementation for those protocol state machines. Tonic is a gRPC
framework built above Hyper and is not a dependency for Node's HTTP APIs.

Hyper normally drives an `AsyncRead + AsyncWrite` connection continuously. Kiln's stable HostOps
seam deliberately exposes discrete `TcpRead` and `TcpWrite` requests so live and scripted providers
share one request/resource/completion protocol. Passing a raw Tokio socket directly to Hyper would
bypass scripted I/O, resource authority, cancellation, reference accounting, and the realm's sole
completion ingress. Adding HTTP-shaped HostOps would put protocol policy below the wrong boundary.

## Decision

### Ownership is split by semantics

- Tokio owns task polling, readiness, sockets, timers, and cancellation.
- Hyper owns HTTP/1 and HTTP/2 parsing, framing, connection state, and protocol errors.
- The realm owns JavaScript ordering and Node semantics: `EventEmitter`, readable and writable
  streams, backpressure, headers, errors, request/response objects, and lifecycle events.
- HostOps owns external authority and portable resource/completion records. It does not expose a
  Tokio, Hyper, or JavaScript type.

There is one Tokio runtime and one realm completion ingress. Hyper connection futures run as local
tasks on the realm's existing current-thread runtime. They never call JavaScript directly; decoded
protocol events are queued back to the realm with the ordinary monotonic submission sequence.

### One byte-transport bridge serves live and deterministic execution

The runtime owns a private duplex byte channel for each HTTP connection. Its Hyper-facing half
implements Hyper's Tokio I/O traits. Its realm-facing half translates demand and produced bytes to
the existing `StreamDriver`/HostOps protocol:

    Hyper poll_read  -> one pending TcpRead  -> sequenced Completion -> wake Hyper
    Hyper poll_write -> StreamDriver write  -> TcpWrite Completion  -> wake Hyper
    EOF/error/close  -> resource lifecycle  -> Hyper result         -> realm event

At most one transport read and one transport write are in flight per connection. Node's
high-water marks and `drain` remain properties of `StreamDriver`; Hyper's buffers do not become a
second user-visible backpressure model.

`TokioHost` supplies live TCP completions. `ScriptedHost` supplies recorded completions through the
same operations and identities. Deterministic conformance therefore runs the real Hyper connection
future on paused Tokio time without binding a port or installing a mock executor.

The bridge represents an attached connection as `HttpResourceId` and publishes
`HttpStreamAction`, not the generic Node stream action. HTTP may select typed read, write, and
shutdown operations directly, but the action type has no raw `CloseResource` operation. A
graceful or abort transition instead selects a typed close marker; one runtime-owned adapter is
the only place that can lower that marker to the unchanged HostOps close operation and register
its completion. This nominal boundary makes bypassing Hyper's drain/abort lifecycle a Rust type
error while preserving the provider and C ABIs. The runtime state-machine tests still prove the
temporal property that queued and in-flight writes complete before a graceful close.

### Migration is event-shaped, not API-shaped

The Node adapters consume Kiln-owned request/response events rather than Hyper types. During
migration, the existing parser and Hyper can each produce that event vocabulary, allowing
interpreter and native paths to move independently without changing JavaScript bindings twice.
When both paths use Hyper, the handwritten parser and encoder are deleted.

Hyper, `http`, `bytes`, and body helper versions and enabled features are part of the shipped
runtime package identity. They do not enter compiler IR, module records, generated-module ABIs, or
the public embedding interface.

### Dependency surface stays narrow

Use Hyper with default features disabled and enable only the client/server and protocol features
actually shipped. Use `hyper-util` only for its Tokio I/O adapter where required. Do not enable a
second executor, DNS client, TLS stack, proxy implementation, or Tonic as part of this decision.
Those capabilities require their own Node-visible policy and tests.

## Consequences

- Kiln deletes protocol machinery as Hyper-backed paths replace it instead of growing a second
  HTTP implementation.
- Scripted and live executions exercise the same HTTP engine and Node adapters.
- HTTP/2 becomes an extension of the connection engine rather than another generated-C parser,
  while Node's exposed negotiation policy remains explicit work.
- The bridge is runtime-private. Embedders continue to implement services and capabilities, not
  Rust futures or Hyper traits.
- A Hyper task becoming ready is not JavaScript ordering evidence. Realm sequence numbers and
  checkpoints remain authoritative.

## Alternatives rejected

**Pass Tokio sockets directly to Hyper.** This is shorter for live execution and silently creates a
second path that deterministic HostOps, permissions, resource accounting, and embedders cannot
exercise.

**Add HTTP operations to HostOps.** This makes an environment provider implement protocol policy
and prevents the same Node runtime from being tested against byte transcripts.

**Keep extending the handwritten parser.** The tracer bullet has served its purpose. Matching
Hyper's security, framing, and connection coverage is not a useful source of Kiln-specific value.

**Use Tonic.** Node HTTP compatibility is not gRPC. Tonic may be used later by a package that
implements gRPC semantics, above this boundary.
