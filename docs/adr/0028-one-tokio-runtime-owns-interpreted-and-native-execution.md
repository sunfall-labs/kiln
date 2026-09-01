# ADR 0028: One Tokio runtime owns interpreted and native execution

## Status

Accepted. This supersedes proposed ADR 0018.

It refines ADR 0009 by replacing the generated C host runtime with a shipped Rust runtime while
keeping separately compiled, separately cached C application units. It supersedes ADR 0033's
per-translation-unit microtask-queue decision; that approximation remains the implementation until
this migration reaches the native backend. ADRs 0012 and 0015 still define the realm's observable
clock, and ADR 0025's rule that language engines are linked rather than reimplemented still stands.
This ADR does not choose the internal value representation or collector left open by proposed ADR
0021.

Implementation milestone (31 August 2026): `kiln-runtime` owns the deterministic Realm clock,
microtask queue, timer ordering and native HTTP transport ingress, and interpreted bytecode enters
that runtime. Linked interpreted modules reuse one runtime instance. Native executables create an
opaque Realm through the private Rust archive, enter every generated-C module body as a callback on
its Tokio runtime, submit opaque Promise/`queueMicrotask` and timer work through the private ABI,
drive the Realm after the scalar program or complete module graph, and then destroy it. The former
per-translation-unit queue in ADR 0033 is gone. Native `process.nextTick` is the remaining callback
lane: its FIFO must live beside the Realm microtask queue and drain before ordinary microtasks at
every checkpoint.

## Context

At decision time Kiln had two implementations of one runtime. The interpreter owned a virtual
clock, timer table, promise jobs and microtask queue in Rust. The native backend still emits the
corresponding heap, promise machinery, timers and event loop as C. Differential tests keep the two
paths aligned during migration, but agreement remains an obligation until that C loop is removed.

That duplication has already produced a known semantic boundary. The native microtask queue is per
translation unit, so a module drains its jobs before the next module body runs. ECMAScript requires
one queue for the realm, drained after module evaluation reaches the appropriate checkpoint. ADR
0023 records the current behaviour as correct for one module and approximate for a graph.

The HostOps boundary is synchronous today because its only operation is writing stdout. Node
compatibility requires timers, files, sockets, subprocesses, signals, cancellation and streams.
Those are asynchronous services, and implementing a separate executor, reactor and cancellation
model for them would duplicate infrastructure unrelated to JavaScript semantics.

The linked semantic provider proves the distribution model. Kiln already builds Rust as a PIC
static library, describes it with a strict target manifest, includes its identity in artifact keys,
and links it into emitted C executables and shared libraries. It supplies number conversion,
regular expressions and arbitrary-precision integers to both execution paths from the same Rust
implementations. A Rust runtime continues to run after the compiler exits; the real distribution
constraint is availability of a compatible archive for the artifact's target.

## Decision

### Tokio is the runtime, not a selectable driver

Kiln uses Tokio directly as its asynchronous execution substrate. There is no pluggable event-loop,
reactor or timer-driver abstraction beneath the realm. Tokio owns task polling, timer wakeups,
asynchronous I/O, cancellation and runtime shutdown in both interpreted and native execution.

The runtime crate opts into only the Tokio features it uses. Tokio is isolated below the stable
runtime and C ABI boundaries so its types do not enter compiler IR, cache records, generated module
interfaces or the public embedding ABI.

Compiler queries and native code generation remain ordinary synchronous CPU work. Work that can
block a runtime thread, including toolchain processes and blocking host libraries, uses Tokio's
process or blocking facilities rather than running inside a realm task.

### A realm is one local task

Each realm is owned by exactly one local task on a current-thread Tokio runtime. JavaScript heap
state is not `Send`, does not move between worker threads, and is never accessed concurrently.
Multiple realms may share a runtime thread when their ownership remains disjoint; additional
runtime threads create additional current-thread runtimes rather than moving a live realm.

Tokio decides when the realm is polled. The realm decides which JavaScript continuation runs next.
Promise reactions and `queueMicrotask` callbacks are not individually spawned as Tokio tasks,
because Tokio deliberately does not promise the scheduling order ECMAScript exposes. They remain a
FIFO queue owned by the realm. Timers retain an explicit monotonic sequence number so equal
deadlines run in creation order.

The runtime arms one Tokio timer for the realm's earliest deadline. When it wakes, the realm selects
due timers by `(deadline, sequence)`, runs one callback, drains microtasks to exhaustion, performs
the unhandled-rejection checkpoint, and repeats. This is language state, like frames and the operand
stack, rather than a second general-purpose executor.

### Interpreter and native code enter the same runtime

The interpreter calls the runtime through an ordinary Rust API. Generated C calls it through a
private, versioned C ABI containing opaque runtime, realm, request and continuation handles. No
Rust layout, future, waker or Tokio type crosses that ABI.

An asynchronous native operation returns a request identity. Completion queues an opaque
continuation on its realm; it never calls generated code concurrently or on an arbitrary I/O
worker. Generated async functions remain compiler-produced state machines and resume on the realm
thread through registered C function pointers and caller-owned state.

Private async ABI version 2 carries the generic realm host protocol. A fixed-width operation frame
names the service, resource identity, scalar argument, and borrowed text/byte spans; the Rust
provider copies every span before submission returns. Completion frames carry owned bytes,
filesystem metadata, resource and lifecycle state, accepted-stream peer data, or structured errors.
The continuation sees borrowed payload pointers only during its callback. `ReadFile` remains a
source-compatible wrapper over this generic submission entry while generated Node bindings move to
the resource protocol. Both entries are declared only in generated C units and the private runtime
package, not in the public embedding header. The provider export list and runtime artifact identity
therefore change without changing the public embedding ABI.

The native launcher creates the realm before evaluating any module, gives every module the same
realm handle, and drives that realm after the complete graph reaches its checkpoint. Microtasks,
timers and async completions are therefore realm-wide rather than per translation unit. This is the
correction ADR 0033 reserved.

### Runtime ownership is explicit

A standalone executable owns its runtime and drives it until no referenced realm work remains. The
resident daemon owns a long-lived runtime rather than constructing one per warm interpreter run.

A runtime created through the C embedding ABI has explicit ownership and thread affinity. Starting,
polling or dropping an embedded Kiln realm never implicitly constructs a nested Tokio runtime and
never assumes that an ambient Tokio handle exists. The ABI prototype decides whether a C embedder
drives a nonblocking runtime handle or asks Kiln to own a runtime thread; both forms keep completion,
cancellation and shutdown explicit, and neither changes realm ordering.

Inside Rust, the provider trait makes shutdown a required implementation obligation. The realm
runtime closes admission first, aborts protocol transports and their correlated requests, then asks
the provider to cancel all remaining work and release resources. Explicit shutdown and `Drop` share
that idempotent path; dropping the Tokio executor is not a substitute for it.

Rust-only embedders may use an internal asynchronous API with an existing Tokio handle, but that is
not the stable C contract and cannot change its observable scheduling rules.

### Determinism uses the same runtime

ADRs 0012 and 0015 continue to govern existing execution modes. A deterministic realm starts Tokio
time paused, anchors the observable clock at the Unix epoch, uses UTC, and lets time advance only
when no ready work can make progress before the next timer. A timer an hour away therefore costs no
wall-clock hour.

Determinism is runtime configuration, not a second driver implementation. The interpreter,
Test262, native conformance programs and build-time evaluation use the same realm scheduler and
paused-time policy. Operations whose results depend on ambient network, filesystem or process
timing are unavailable in deterministic mode unless the HostOps provider supplies a recorded or
scripted completion. Adding a live-clock execution mode is a separate observable-language decision;
adopting Tokio does not silently add one.

Tokio's task order is not treated as semantic input. Tests assert ordering at the realm boundary,
and the realm's explicit queues and sequence numbers are authoritative even if Tokio changes its
internal scheduler.

### The shipped runtime is a target artifact

Supported targets receive a versioned Rust runtime package built in CI. The package contains the
link inputs and a strict manifest naming the complete Rust target triple, runtime and private ABI
versions, Tokio version and enabled features, Rust toolchain and source identity, CPU policy, PIC
and linkage mode, deployment and libc baseline, exported symbols and payload digests.

`native-dev` uses the shipped dynamic runtime so an application-unit edit does not relink unchanged
runtime code. `native-release` links the same runtime statically. Embedding packages use an explicit
export allowlist; Tokio, Rust implementation symbols and the private continuation ABI do not become
public embedding symbols.

Resolution uses a shipped compatible runtime first. A source build is allowed only when the exact
Rust target is installed. Otherwise Kiln reports a deterministic unsupported-target error listing
the shipped target matrix. A supported user does not need Rust installed to compile a Kiln native
artifact.

Runtime package identity participates in the runtime artifact key and final link key, but not in a
generated application unit's object key. ADR 0009's layered reuse therefore survives: changing the
runtime relinks an application without recompiling its unchanged modules.

The existing `kiln-native-number-ffi` archive is a migration slice of this runtime distribution.
Number conversion, `regress` and `num-bigint` move into the complete runtime package rather than
remaining a second permanent Rust archive.

### Failure and ownership do not leak through FFI

Every exported C entry catches Rust unwinding and converts it into a runtime or realm failure before
control returns to C. No panic unwinds across the ABI. A panic in a spawned runtime task fails its
realm and cancels its remaining work instead of being ignored. Allocation failure is process-fatal
in the first runtime ABI and is documented as such for embedders; pretending an infallible Rust
dependency graph can recover from it would be a false contract.

Rust engines may retain opaque resources behind runtime handles, but those handles have an explicit
owner and disposal operation and are released by realm teardown even if JavaScript never observes a
finalizer. Host buffers are copied or remain borrowed only for the documented duration of one ABI
call. This ADR does not decide where JavaScript payloads live or which collector owns them; those
remain ADR 0021's questions.

HostOps remains the boundary for services supplied by an environment. Language semantics such as
regular expressions and BigInt remain runtime responsibilities under ADR 0025. HostOps evolves
additively from synchronous calls to request/completion operations; an embedder supplies services,
not an executor and not a JavaScript engine.

## Consequences

- Kiln has one executor, reactor, clock mechanism and cancellation model across its Rust-hosted and
  native paths.
- The interpreter and native backend share one realm-level scheduling contract, and the native
  graph no longer has a per-module microtask approximation after migration.
- Real asynchronous HostOps can be added without inventing a Kiln-specific futures runtime or
  exposing Tokio to generated modules and embedders.
- Generated C application units, their independent cache keys and the portable C compiler boundary
  remain. The shipped Rust runtime is an additional target link input, not a replacement backend.
- Native target support becomes an explicit release matrix. Adding a target requires producing and
  testing its runtime package rather than merely finding a C compiler for it.
- Tokio increases the shipped runtime's size and cold-start surface. Both are measured in the
  existing native profiles; they are accepted only once per target runtime, not duplicated into
  every application unit.
- The runtime crate becomes high-leverage infrastructure and must pin Tokio through `Cargo.lock`,
  test deterministic ordering across upgrades, and record its ABI and feature identity in packages.
- The current generated C loop and private Rust semantic-provider archive remain temporary
  migration components. They are removed only after interpreter/native differential coverage runs
  through the shared runtime.

## Alternatives rejected

**Keep independent Rust and C event loops.** This preserves the current C-only target story but
retains duplicated semantics, blocks a shared asynchronous HostOps model, and leaves the known
cross-module queue approximation in place.

**Spawn every JavaScript job as a Tokio task.** This appears to let Tokio own more, but makes an
unspecified executor order observable as JavaScript behaviour. One realm task with explicit
language queues gives Tokio all infrastructure responsibility without delegating language meaning.

**Put Tokio behind a driver trait.** Kiln has no second production reactor to select. An abstraction
would add two APIs and a mock implementation while the paused Tokio runtime already supplies the
required deterministic clock. The HostOps boundary remains sufficient for scripted external
completions.

**Use libuv for native code and Tokio for Rust code.** Both are mature, but selecting both recreates
the duplicate-runtime problem this decision removes and requires two cancellation, timer and I/O
integration surfaces.

**Adopt an existing JavaScript engine.** V8, QuickJS or Boa would bring its own job machinery, but it
would also replace Kiln's bytecode interpreter, generated C semantics and compiler/runtime boundary.
That is a different project rather than a runtime implementation choice.

## Sources

Checked 30 August 2026 for the Tokio properties this decision relies on.

- Tokio runtime scheduling guarantees and the explicit absence of a task-order guarantee:
  <https://docs.rs/tokio/latest/tokio/runtime/>.
- Paused current-thread time and automatic advancement to the next timer when no other work can
  progress: <https://docs.rs/tokio/latest/tokio/time/fn.pause.html>.
- `LocalSet` ownership of `!Send` tasks on one thread:
  <https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html>.
