# ADR 0064: Pluggable Linux io_uring completion engine for high-throughput HTTP

## Status

Proposed. Extends ADR 0032 (Hyper/Tokio ownership split), ADR 0046 (Hyper tasks over host transports),
and ADR 0058 (Rust-backed runtime engines).

## Context

Following the WHATWG optimizations in `crates/kiln-builtins` and the first-class `serve()` API, Kiln
Native achieves **61,966 req/sec** on raw HTTP and **19,910 req/sec** on direct fetch handlers on a single
CPU core, outperforming Node.js (44,613 RPS) by +38.9% and Bun (57,130 RPS) by +8.5% on the raw protocol.

However, to compete with the fastest network servers in the world (such as `uWebSockets`, `Drogon`, and
`Actix-web`, which reach 150,000–300,000 req/sec per core and millions of req/sec multi-core), the runtime
must overcome the architectural ceiling of traditional Linux `epoll`:

1. **Syscall Overhead & Context Switching**:
   Under traditional `epoll`, every request incurs multiple kernel transitions (`epoll_wait`, `read`, `write`).
   At 100,000+ RPS, CPU time is dominated by syscall context switching and kernel socket lock contention.
2. **Readiness vs. Completion Mismatch**:
   Tokio and Hyper operate on the Unix *readiness* model (`AsyncRead`/`AsyncWrite` with borrowed `&mut [u8]`).
   Data is copied from kernel socket buffers (`sk_buff`) into userspace memory on every read, and vice-versa
   on every write.
3. **The Kernel Solution (`io_uring`)**:
   Modern Linux kernels (>= 5.10, especially 6.x) offer `io_uring`:
   - Shared memory ring buffers (Submission Queue and Completion Queue) eliminating syscalls in the steady state (`IORING_SETUP_SQPOLL`).
   - Pre-registered, pinned memory buffers (`IORING_REGISTER_BUFFERS`) bypassing page-table lookups.
   - Zero-copy socket transmission (`IORING_OP_SEND_ZC`).
   - Multi-shot connection acceptance (`IORING_ACCEPT_MULTISHOT`).

Evaluating this against Rust runtime libraries brings up `monoio` (ByteDance's thread-per-core `io_uring`
runtime) and its companion `monoio-http` engine.

## Evaluation: Can Monoio Provide Everything Kiln Needs?

An objective audit of `monoio` and `monoio-http` reveals both immense strengths and critical constraints:

### Strengths
- **Raw Network Performance**: In microbenchmarks, `monoio` consistently achieves 2× to 3× the raw I/O
  throughput of standard Tokio on Linux.
- **Zero-Copy Architecture**: Built natively around owned buffer rental (`AsyncReadRent`/`AsyncWriteRent`),
  making it a direct fit for `IORING_REGISTER_BUFFERS` and zero-copy TCP sends.
- **Production Provenance**: ByteDance runs `monoio` and `Monolake` (its service gateway) at immense scale
  (tens of millions of queries per second).
- **Thread-Per-Core Alignment**: Kiln’s multi-worker launcher (`KILN_WORKERS=auto` via `SO_REUSEPORT`,
  ADR 0038/ADR 0046) already assigns an independent realm per OS process. This matches `monoio`’s shared-nothing,
  thread-per-core model.

### Constraints & Deficiencies
1. **Linux-Only Primitive**:
   `io_uring` is strictly Linux. While `monoio` has an `epoll` fallback on Linux, its support for macOS
   (`kqueue`) and Windows is experimental and lacks the maturity of Tokio's `mio`. Replacing Tokio entirely
   would severely degrade macOS developer workflows and Apple Silicon profiling (`scripts/profile-native.sh`).
2. **Broader Node.js Ecosystem Seams**:
   While `monoio` excels at TCP and file I/O, other Node compatibility subsystems (`node:child_process`,
   asynchronous DNS, IPC channels, complex signal handling) are deeply tied to Tokio’s comprehensive ecosystem.
3. **Deterministic Virtual Clock & Scripted Host (ADR 0012, ADR 0046)**:
   Kiln’s test suite and conformance runner depend on `ScriptedHost` and `ScriptedTransport` operating on a
   virtual clock. Any alternative engine must allow mocked, deterministic duplex streams without requiring
   real kernel rings.

## Decision

Kiln will **not** replace Tokio workspace-wide. Instead, Kiln adopts a **pluggable provider architecture**:

### 1. Tokio + Hyper Remains the Universal Baseline
Tokio and Hyper remain the default, universally conformant engine for:
- All platforms (macOS, Linux, Windows).
- Development builds, test suites, and Test262 conformance runs (guaranteeing deterministic virtual time).
- General-purpose Node APIs (`child_process`, `fs`, `dns`, IPC).

### 2. Introduce a Pluggable `HttpServerEngine` Seam for Linux (`monoio-http`)
In `crates/kiln-runtime`, abstract the HTTP server transport behind an engine capability:
- Under `--features=linux-uring` (or an explicit `--uring` build flag on Linux targets):
  `HttpServerEngine` activates a `MonoioHttpServer` implementation backed by `monoio` and `monoio-http`.
- The engine pre-registers a dedicated pool of pinned memory pages (`IORING_REGISTER_BUFFERS`) and uses
  multi-shot accept (`IORING_ACCEPT_MULTISHOT`).
- Requests are dispatched into the C realm via the existing `HttpDispatch` trait without intermediate
  `IncomingMessage` dictionary object churn.

### 3. Synergistic Scaling with `KILN_WORKERS=auto`
When deployed in production on Linux:
- `KILN_WORKERS=auto` creates one `monoio` worker per physical CPU core.
- Each worker binds its `io_uring` ring to the shared port using `SO_REUSEPORT`.
- Single-core throughput increases from 62k RPS to **150,000–250,000+ RPS**, scaling linearly across cores
  to exceed **1,000,000+ requests per second**.

## Consequences

- **Cross-Platform Safety**: macOS and non-uring systems retain 100% of their existing stability, profiling
  tools, and conformance guarantees.
- **Opt-In Peak Performance**: High-throughput server deployments on Linux can compile with `io_uring` to
  eliminate syscall overhead and match the fastest C++ servers in the world.
- **Provider Isolation**: The C language runtime and JavaScript builtins remain completely agnostic to
  whether the underlying packets were harvested from Tokio's `epoll` or Monoio's `io_uring`.
