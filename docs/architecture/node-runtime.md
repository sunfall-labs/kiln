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

## The dependency rule

Node compatibility is four layers, and dependencies point only downward:

```text
Node module facades
    node:http, node:net, node:fs, process, Buffer
                         │
                         ▼
Node semantic machines
    streams, EventEmitter, HTTP lifecycle, error translation
                         │
                         ▼
Realm host protocol
    operations, owned payloads, resources, requests, cancellation, liveness
                         │
                         ▼
Providers
    Tokio production services | scripted deterministic services
```

The upper two layers know Node; the lower two do not. A host operation is named for the service it
performs (`tcp_accept`, `read`, `stat`), never for the JavaScript method that requested it
(`http_server_listen`, `readFile`). Conversely, Tokio types and readiness never enter a Node
semantic machine. This makes an HTTP client, an HTTP server, `net.Socket`, and future TLS reuse one
transport/resource implementation while retaining their distinct Node-visible behavior.

The implementation may initially colocate these layers in one crate, but it must preserve this
dependency rule in its types and modules. The intended steady-state crate split is:

- `kiln-host-abi`: dependency-free realm host protocol and owned boundary values;
- `kiln-runtime`: realm scheduler, resource/request ownership, and Tokio production provider;
- a Node adapter crate: reusable Node semantic machines and builtin facades;
- interpreter/native bindings: representation-specific conversion and continuation registration.

`NodeHostOps` is therefore a migration name, not a second permanent boundary. Persistent resources
belong in the generic asynchronous realm host protocol. Once that protocol can start resource
operations in the active realm context, the Node-specific trait is deleted rather than expanded.

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

HTTP client connection reuse belongs to a realm-owned Agent above the protocol engine. The Agent
keys pools by origin and transport policy, assigns realm-scoped request identities, queues requests
while a connection is busy, and retires idle or failed resources. Hyper owns each connection's
HTTP state but does not choose which origin may reuse it; Tokio owns polling but does not choose
JavaScript-visible Agent lifetime. Interpreter and generated-native bindings consume the same
acquire/release decisions rather than maintaining representation-specific socket caches.

### One completion ingress

Every asynchronous service follows one lifecycle:

```text
submit operation → RequestId → provider future → owned completion
    → realm checkpoint → language continuation → JavaScript job queues
```

There are no callbacks from an I/O task into the interpreter, no per-builtin completion queues,
and no direct polling of sockets from JavaScript objects. A completion may update resource state,
but it can make JavaScript observable only by entering the realm's single completion ingress. This
is the seam shared by interpreted bytecode and generated native continuations.

Requests and resources are realm-scoped opaque identities. Cancellation is idempotent, late
completions are discarded by identity, and teardown cancels requests before closing resources.
The provider may execute independent operations concurrently, but each stream admits at most one
read and preserves write order at its semantic machine. Backpressure is an explicit state
transition, not an emergent property of Tokio channel capacity.

### Liveness is semantic state

Tokio decides whether infrastructure can make progress; it does not decide whether a Node program
should keep running. Timers, requests, and resources carry an explicit referenced bit. The realm
terminates only when its language queues are empty and it owns no referenced work. `ref()` and
`unref()` mutate that bit without moving or recreating the underlying operation.

Shutdown is structured and realm-owned: stop admitting work, cancel outstanding requests, close
resources, drain cancellation acknowledgements that affect language state, then drop the provider.
Dropping a Tokio runtime is never used as the observable cancellation protocol.

## Determinism

Determinism is a realm configuration, not a second runtime. The default capability policy uses a
virtual epoch clock, the ADR 0026 seed, UTC, and `kiln-intl`'s default locale. A deterministic host
script declares each expected operation and completion. Unexpected operations fail with their
request identity and operation, so a corpus cannot silently touch the machine.

Live mode uses the same queues and completion-order rules. Tokio may complete work in any order;
the realm turns ready completions into JavaScript turns using explicit sequence numbers. Network
payload ordering comes from each stream, while unrelated ready resources use request sequence as
the deterministic tie-break.

Determinism has two independent parts and neither is allowed to impersonate the other:

- realm capabilities choose observable clock, entropy, timezone, and locale policy;
- a host provider chooses which external operations exist and when their completions are released.

A deterministic realm can therefore execute a fully realistic HTTP state machine against scripted
TCP byte streams. A live realm can still choose seeded entropy or a named timezone. Provider choice
is execution policy and must be explicit at realm construction; capability choice remains part of
artifact identity as ADR 0031 specifies.

The script is an ordered transcript of submitted operations and release checkpoints, not a bag of
stubbed return values. It validates operation kind and arguments, assigns ordinary `RequestId`s and
`ResourceId`s, models cancellation and partial reads/writes, and fails closed on an unexpected or
unconsumed entry. That lets the same compatibility case exercise fragmentation, backpressure,
half-close, cancellation races, and equal-checkpoint completion ordering reproducibly.

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

Compatibility claims are promoted in vertical slices. Each slice must cross the production module
resolver, builtin facade, Node semantic machine, realm host protocol, and provider. The minimum
evidence for a promoted server slice is:

1. a pinned/adapted upstream Node behavior case;
2. a deterministic scripted-provider case covering adverse fragmentation or ordering;
3. a live loopback case through the Tokio provider;
4. the self-driving real-package/server probe when the slice is on its dependency path.

Native execution becomes another binding of the same slice, not a separate implementation target.
No Node API is advertised on native merely because its synchronous facade compiles; it is advertised
only when its continuations, resources, cancellation, and liveness enter the shared realm protocol.

## Migration sequence

The architecture is migrated by deleting alternate ownership, not by building a parallel stack:

1. finish the generic async realm-host protocol and fold `NodeHostOps` into it;
2. make one realm pump arbitrate microtasks, `nextTick`, timers, host completions, and liveness;
3. move stream/net/HTTP semantic machines above `kiln-runtime`, leaving Tokio transport below;
4. implement the strict scripted provider and run the same state machines against both providers;
5. attach generated native continuations to the same completion ingress and delete the generated-C
   queues, timers, and event-loop ownership (the generic operation/resource ABI and shared
   completion-vs-timer arbitration are complete; Node facade bindings remain);
6. broaden the pinned Node inventory by real-server leverage, promoting only complete claims.

At every step there is one authoritative owner for each concern. Temporary compatibility shims get
a deletion condition in the step that introduces them.

## Architectural acceptance tests

The following invariants are release gates, not documentation aspirations:

- engine and Node adapter crates contain no ambient clock, entropy, locale, filesystem, socket, or
  process reads;
- no Tokio, Future, waker, file descriptor, or Rust layout crosses the host or native ABI;
- Tokio tasks never invoke JavaScript or mutate the JavaScript heap;
- every external completion enters through the realm checkpoint and has a request identity;
- equal-checkpoint completion order is stable, while per-stream byte order is preserved;
- teardown leaves no requests or resources and late completions cannot resurrect a realm;
- the default realm and ordinary conformance corpus remain deterministic;
- live and scripted providers pass the same transport/stream/protocol semantic suites;
- interpreter and native paths do not advertise different Node surfaces.

CommonJS is a module-graph source kind, not a resolver alias and not an ESM default-export patch.
The Node resolver selects the file and its nearest package type; the graph records CommonJS
identity; evaluation supplies `require`, `module`, `exports`, `__filename`, and `__dirname` with a
per-realm module cache that inserts before execution for cycles. ESM import observes
`module.exports` as the default namespace value. This keeps package policy, linkage, execution,
and cache lifetime explicit and prevents framework-specific source rewriting.

## Non-goals

- Reimplementing an executor, reactor, TCP stack, or TLS library.
- Making each JavaScript job a Tokio task.
- Exposing ambient host state to engine crates.
- Claiming native Node compatibility before native continuations and async resources use this same
  runtime boundary.
