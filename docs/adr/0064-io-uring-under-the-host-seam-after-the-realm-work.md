# ADR 0064: The HTTP request is realm work first; io_uring goes under the host seam

## Status

Proposed. Settled by the project owner on 2026-09-26 as waiting on the implementation branch;
that branch (`perf/http-fast-path`) has since landed with the measurements below, so this ADR is
ready to be accepted or rejected.

This revision (2026-09-26) replaces the same-day draft titled "Pluggable Linux io_uring
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
* **Selected at run time, not compile time.** `KILN_IO` unset or `auto` selects io_uring on Linux
  6.1 and newer, the first LTS kernel with everything the transport uses, and epoll below it. The
  host probes the ring on first use and falls back to epoll on `ENOSYS`, `EPERM` (Docker's
  default seccomp profile), `kernel.io_uring_disabled`, a missing opcode or a failed buffer-ring
  registration. The floor exists because the probe only catches a kernel that refuses something:
  the transport's first design, DEFER_TASKRUN plus an eventfd, was accepted by 7.2, passed every
  functional test, and added 6–8 ms to each keep-alive request. `KILN_IO=epoll` forces epoll;
  `KILN_IO=uring` asks for the ring below the floor. One provider archive, one test matrix.

### 3. Measure CPU per request

A change to the HTTP path is judged by server CPU per request on a pinned core, interleaved
against its base. Throughput figures belong next to a load client that is shown to saturate the
server.

## Measured results

Implemented on `perf/native-serve`, `perf/lazy-node-http` and `perf/uring-transport`, integrated on
`perf/http-fast-path`. The machine was shared with other builds throughout (load average 15–34),
which roughly triples the absolute figures against the quiet-machine profile above. The ratios
come from interleaved runs of base and candidate in the same minutes.

**Realm work.** Server µs/request, three interleaved rounds, node load client, one pinned core:

| | ca3b20a8 | integrated | change |
| --- | --- | --- | --- |
| `node:http` hello | 30.4 / 32.7 / 31.1 | 29.6 / 27.6 / 28.1 | −10% |
| `serve()` hello | 79.6 / 71.3 / 67.8 | 34.7 / 32.9 / 32.3 | 2.2x less |
| `serve()` route (reads URL and a header, sets headers) | 85.1 / 87.0 / 80.4 | 61.6 / 55.5 / 52.5 | 1.5x less |

* `serve()` no longer builds node objects. The engine dispatches whole requests, the `Request`
  builds its URL and headers on first read, and the response reaches the `ResponseSlot` in one
  crossing (`fetchRespond`).
* `node:http` builds `req`/`res` from fixed layouts with pre-interned keys. It stopped recycling
  one pair per keep-alive socket, which cost nine property deletes per request and was the
  source of three native-only bugs: a kept `req` changed `url` on the next request, handler
  properties leaked across requests, and `res.statusCode = 404; res.end()` sent 200. User
  instructions per request fell 35%.
* `req.headers` kept a raw pointer into the engine's borrowed per-connection header buffer, so
  any read after an `await` saw the next request's headers or freed memory. The head dispatch
  now copies the bytes. `conformance/native/app-http-request-lifecycle.mjs` covers this.
* Class instances whose prototype chain has a getter had become dictionary objects on the native
  backend, and the root shape stopped branching at 32 class brands, which the bootstrap had
  already used up. Both are fixed (7a2ae86d), and the fix applies to all native code.

What remains in `serve()` route is general native object-model cost rather than HTTP: a missing
property read is ~350 ns, an object literal ~1.4 µs, `new Headers(obj)` ~2.2 µs, and a `for…of`
over a Map ~5 µs.

**io_uring.** Median of eight interleaved runs, `node:http` hello, same binary, `KILN_IO`
switched:

| load | epoll | io_uring |
| --- | --- | --- |
| oha, 16 connections | 17.6 µs, 35k req/s | 10.3 µs, 61k req/s |
| oha, 64 connections | 16.4 µs, 42k req/s | 9.4 µs, 71k req/s |
| node load client | 30.7 µs | 29.4 µs |
| one connection, p50 latency | 34 µs | 36 µs |

At 64 connections io_uring makes 0.07–0.09 syscalls per request (about 26 requests per
`io_uring_enter`) against epoll's 2.04. Syscall entry/exit falls from 4.2% of CPU to 0.3%, and
the ring and its driver add about 5%. `KILN_WORKERS=4` with four rings served 190k req/s. The
gain appears only when the server is busy enough to batch. Under the node client, which cannot
saturate the server, the two are within noise.

## Consequences

* The draft's 150–250k req/s per core and 1M req/s claims are withdrawn. Past the io_uring
  batching, the ceiling the profile supports is set by the kernel TCP path. On loopback that is about 5 µs of the 10.7;
  on a real NIC without conntrack it is roughly 3 µs.
* io_uring is Linux-only by construction and selected at run time. macOS, Windows, Linux below
  6.1 and any Linux host that refuses the ring keep the Tokio transport. Only 7.2 has been
  validated; a regression on another kernel is answered with `KILN_IO=epoll` and a raised floor.
* Each realm on the ring reserves 1 MiB of provided receive buffers.
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
