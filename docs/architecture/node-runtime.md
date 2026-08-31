# Node runtime architecture

This document is the implementation map for ADRs 0028 and 0031. It exists to keep Node
compatibility from turning into a growing set of built-ins that each discover their own executor,
resource lifetime, determinism switch, or host dependency.

## Ownership

One `RealmRuntime` owns one current-thread Tokio runtime, the realm's resource table, asynchronous
request table, and environmental capability policy. JavaScript heap state stays on its one realm
task. Tokio polls infrastructure futures; explicit realm queues order JavaScript jobs.

The runtime also owns one Tokio `LocalSet`. Realm futures use `spawn_local`: socket handles and
JavaScript continuations remain non-`Send`, matching the realm's thread affinity instead of adding
cross-thread synchronization merely to satisfy `Handle::spawn`. Background futures return private
completion outcomes; only the realm owner allocates public `ResourceId`s or mutates the resource
table.

There is no event-loop driver trait and no second executor. There is one service boundary:
`AsyncHostOps`. Production implements it with Tokio filesystem, socket, process, signal, and DNS
facilities. Deterministic conformance implements it with a script of requests and completions. The
script is a provider, not an executor: Tokio still owns polling and realm wakeups.

`kiln run --node` is the explicit product boundary. It selects the Node resolver, typed builtin
graph nodes, adapter surface, cache identity, and Tokio-backed host provider as one coherent mode.
Without the flag, `node:` specifiers fail closed; with it, unknown builtins still fail closed. A
module cannot accidentally acquire Node semantics because a dependency happened to use Node syntax.

```text
Interpreter / generated continuation
              │ opaque request + continuation
              ▼
       RealmRuntime (one owner)
       ├─ language queues: Promise jobs, nextTick, timers
       ├─ resource table: files, listeners, sockets, processes
       ├─ capability policy: clock, entropy, timezone, locale
       └─ AsyncHostOps provider
          ├─ live: Tokio fs/net/process/signal
          └─ scripted: pinned completions for deterministic tests
```

## Three boundaries, three kinds of policy

1. `RealmCapabilities` describes observable environmental policy. Its default is byte-for-byte the
   historical deterministic realm. Host-backed values cross synchronous `HostOps`; no engine reads
   the ambient clock, entropy source, timezone, or locale.
2. `AsyncHostOps` describes asynchronous services and resource ownership. It carries owned values,
   opaque request/resource identities, cancellation, and referenced liveness. It never exposes a
   Future, waker, Tokio handle, file descriptor, or Rust layout.
3. Node adapters implement JavaScript API shape in JS where practical and thin Rust glue where the
   VM needs object integration. `EventEmitter`, streams, Buffer coordination, HTTP parsing policy,
   and errors belong above host operations. Tokio performs I/O; it does not define Node semantics.

These are deliberately composable layers, not one adapter per module: transports produce owned
byte/completion events; protocol codecs turn bytes into HTTP events; stream adapters impose Node's
flow and encoding semantics; module facades expose the JavaScript objects. HTTP client and server
therefore share transport, resource lifetime, event dispatch, and backpressure instead of growing
two loops. Future TLS wraps the transport seam; it does not fork the scheduler.

## Determinism

Determinism is a realm configuration, not a second runtime. The default capability policy uses a
virtual epoch clock, the ADR 0026 seed, UTC, and `kiln-intl`'s default locale. A deterministic host
script declares each expected operation and completion. Unexpected operations fail with their
request identity and operation, so a corpus cannot silently touch the machine.

Live mode uses the same queues and completion-order rules. Tokio may complete work in any order;
the realm turns ready completions into JavaScript turns using explicit sequence numbers. Network
payload ordering comes from each stream, while unrelated ready resources use request sequence as
the deterministic tie-break.

## Artifact identity

Artifact identity records policies that affect execution meaning:

- Node compatibility version and enabled adapter surface;
- clock/entropy/timezone/locale policy;
- caller values for seeded entropy and named timezone/locale.

It never records transient values returned by a host-backed capability. The canonical encoding is
owned by the typed policy itself and is length-framed; cache callers do not assemble strings.
Changing the default policy would intentionally change every key. Adding the capability field with
today's default therefore requires a cache-format bump but changes no semantic policy.

## Resource lifecycle

Every persistent host object is a `ResourceId` owned by one realm. Resources expose explicit close
and reference state. Realm teardown closes all resources and cancels all requests. A referenced
listener, socket, timer, or process keeps the realm alive; `unref()` changes only liveness. A
completion never invokes JavaScript directly—it queues a continuation on the realm.

## Compatibility testing

Kiln vendors a pinned Node test tree separately from its configuration. Like Deno, the checked-in
configuration is an explicit map of test path to expectation: pass, skip with reason,
platform-specific, expected failure, or flaky. CI gates tests promoted to pass. A periodic full run
reports every test, including failures, so compatibility can improve without allowlist drift.

The priority matrix follows real server leverage, not alphabetical module coverage:

1. `events`, Buffer, timers, process lifecycle, URL and errors;
2. `net`, `stream`, HTTP client/server, DNS needed by loopback servers;
3. module/package rules and CommonJS interop needed by Fastify;
4. filesystem, crypto, child process and other APIs used by measured applications.

The self-driving server probes are vertical acceptance gates. Unit compatibility counts never
substitute for running them.

The first gate is `conformance/server/s01-http-hello.mjs`: an HTTP server and HTTP client share one
realm, communicate over loopback, decode a streamed UTF-8 response, close the listener, and let the
realm terminate. Passing it proves the production module graph and runtime path, not merely codec
units or a test-only loader.

## Non-goals

- Reimplementing an executor, reactor, TCP stack, or TLS library.
- Making each JavaScript job a Tokio task.
- Exposing ambient host state to engine crates.
- Claiming native Node compatibility before native continuations and async resources use this same
  runtime boundary.
