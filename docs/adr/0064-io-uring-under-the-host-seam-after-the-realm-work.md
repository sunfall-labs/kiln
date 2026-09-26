# ADR 0064: The HTTP request is realm work first; io_uring goes under the host seam

## Status

Proposed. This revision (2026-09-26) replaces the same-day draft titled "Pluggable Linux io_uring
completion engine for high-throughput HTTP". That draft proposed `monoio` and `monoio-http` behind
a compile-time `linux-uring` feature, on the strength of throughput figures that the load client
had capped and projections that nothing measured. The measurements below replace them.

Extends ADR 0032 (Hyper owns HTTP), ADR 0046 (Hyper tasks over host transports) and ADR 0058
(Rust engines across a coarse provider ABI). Consistent with ADR 0060: one HTTP implementation.

## Context

### The throughput figures could not tell engines apart

The draft reported 61,966 req/s for kiln against 57,130 for Bun and 44,613 for Node on
`bench/server/drive.mjs`. That client is a node process, and on this machine (16 cores, loopback)
it cannot saturate one kiln core. Even at ten client processes the kiln server stayed at 64–78% of
its pinned core, and kiln, Node and Bun all landed at 47–51k req/s. A throughput number taken below
saturation measures the client. The metric this ADR uses instead is **server CPU per request** on
a pinned core, read from `/proc/<pid>/stat`, which the client's speed does not affect.

### Where a request's CPU goes

`perf` with kernel and user stacks, `bench/server/serve.mjs` (hello), natively compiled
`native-release`, one pinned core, HEAD ca3b20a8. **10.7 µs per request**:

| bucket | share | removable by io_uring |
| --- | --- | --- |
| C realm: IncomingMessage / ServerResponse / headers built through the generic property path | 20% | no |
| kernel TCP transmit | 20% | no |
| kernel loopback receive softirq (the *client's* receive path, charged to the sender) | 16% | no; absent on a real NIC |
| Hyper | 8% | no |
| netfilter / conntrack | 7% | no; host configuration |
| syscall entry/exit and speculative-execution mitigations | 6% | **yes** |
| kernel receive copy | 4.5% | no |
| runtime glue and Tokio | 7.5% | partly |
| epoll | 2.4% | **yes** |

The handler itself is 0.4%. Per request the server makes 1.0 `recv`, 1.0 `send` and 0.56
`epoll_wait`. There is no redundant EAGAIN read and no `epoll_ctl` churn, so the readiness model is
already near its syscall minimum. io_uring's ceiling on this workload is therefore the two rows
marked yes, roughly 9%. It removes neither the socket-buffer copy nor the TCP stack.

The `serve()` fetch path is worse: **33 µs per request**, 73% user space. `serve()` was built on
`createServer`. Each request built an IncomingMessage and a ServerResponse, copied them into a
WHATWG `Request`, called the handler, and copied the `Response` back through `setHeader` and
`end`, so it paid for two object models per request.

### What the draft got wrong about the kernel

* `IORING_SETUP_SQPOLL` spends a kernel thread per ring. With `KILN_WORKERS=auto` that is two cores
  per worker, the opposite of thread-per-core.
* `IORING_REGISTER_BUFFERS` serves fixed-buffer read/write. For sockets, the equivalent is a
  provided buffer ring with multishot receive.
* `IORING_OP_SEND_ZC` pins pages and posts a second completion. It loses on small payloads, and a
  15-byte hello body is the worst case for it.
* An io_uring receive still copies out of the socket buffer.

### Why not a second runtime

`monoio` and `compio` are complete executors. `.scratch/compio-http` ran compio beside the realm's
Tokio `LocalSet` and needed a bridge that entered the completion runtime and flushed its driver on
every transport poll, plus special handling to close the listener. The alternative is to move the
realm's drive loop onto the other executor, which means timers, processes, DNS, the Hyper glue and
the paused-clock scripted host (ADR 0012) all move with it. `monoio-http` would also replace Hyper,
giving kiln a second HTTP implementation, which ADR 0060 rules out.

## Decision

### 1. The realm work comes first

The largest share of a request that kiln can remove is in the realm, not the kernel.

* **`serve()` does not construct node objects.** The handler receives a `Request` built straight
  from the dispatched event, and its headers are materialized when first read. A returned
  `Response` with a string or byte body reaches the engine's `ResponseSlot` in one crossing:
  status, header list and body. Streaming bodies keep the general path.
* **`node:http` builds less per request.** Request and response objects get fixed layouts,
  headers are materialized lazily from the event's bytes, prelude literal keys are pre-interned
  (the step ADR 0047 recorded as next), and the common `writeHead`/`end` sequence takes a direct
  path.

Both hold ADR 0058's rule: bytes and handles cross the boundary, and behaviour is identical on
both engines, or the change does not land.

### 2. io_uring is a host transport, not an HTTP engine

On Linux the host can hand Hyper an io_uring-backed transport through the same `take_transport`
seam `TokioHost` and `ScriptedHost` use (ADR 0046). Hyper, `HttpDispatch`, `ResponseSlot`, the
close contract and every scripted test are unchanged.

* **One ring per realm thread, owned by the realm's Tokio runtime.** The ring's readiness is
  registered with Tokio's reactor, so the existing park still waits on timers, filesystem and
  processes as well as ring completions. There is no second executor.
* `SINGLE_ISSUER | DEFER_TASKRUN`, no SQPOLL.
* Multishot accept, and multishot receive over a provided buffer ring. Plain send, with
  zero-copy send only above a measured size threshold.
* **Submission is batched.** Entries queued during a turn are submitted together at one flush
  point in the drive loop, so N connections' receives and sends cost one `io_uring_enter` rather
  than N pairs of syscalls. That batching is what io_uring buys here.
* **Selected at run time, not compile time.** The host probes at startup and falls back to epoll on
  `ENOSYS`, `EPERM` (Docker's default seccomp profile), `kernel.io_uring_disabled` or a missing
  opcode. `KILN_IO=uring|epoll` forces either. One provider archive, one test matrix.

### 3. Measure CPU per request

A change to the HTTP path is judged by server CPU per request on a pinned core, interleaved
against its base. Throughput figures belong next to a load client that is shown to saturate the
server.

## Measured results

To be completed from the implementation branches before this ADR is accepted:
`perf/native-serve`, `perf/lazy-node-http` and `perf/uring-transport`, integrated on
`perf/http-fast-path`.

## Consequences

* The draft's 150–250k req/s per core and 1M req/s claims are withdrawn. The ceiling that the
  profile supports is set by the kernel TCP path. On loopback that is about 5 µs of the 10.7;
  on a real NIC without conntrack it is roughly 3 µs.
* io_uring is Linux-only by construction and optional at run time. macOS and Windows keep the
  Tokio transport, and so does any Linux host that refuses the ring.
* `node:fs` through Tokio's own io_uring driver (Tokio 1.53, `io-uring` feature) would remove the
  blocking-pool hop per filesystem operation. It requires `tokio_unstable`, which the workspace
  keeps out of the shipped runtime. It is left as a separate decision.

## Alternatives rejected

* **monoio and monoio-http behind a compile-time feature.** A second executor, a second HTTP
  implementation and a second archive, all for the 9% the profile allows.
* **compio as the realm runtime.** Possible, but it moves the drive loop, timers, processes and
  the scripted host for the same 9%. Its real advantage is Windows IOCP, which is not this problem.
* **tokio-uring.** Close to the chosen design, but effectively unmaintained, and its sockets are
  not `AsyncRead`/`AsyncWrite`.
