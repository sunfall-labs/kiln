# ADR 0029: Use mature Rust implementations at existing seams

## Status

Accepted.

This refines `design.md` §14's implementation order without editing the founding document. Kiln
will spend local complexity on JavaScript semantics, incremental compilation and the shared
interpreter/native runtime. General-purpose scheduling, package resolution, concurrency testing,
internationalization and host-resource policy should come from mature Rust Modules when they fit
an existing Kiln Interface and preserve deterministic behavior.

## Context

The workspace has reached the point where several small local Implementations are starting to grow
into independent infrastructure projects:

- native building and conformance workers allocate threads, divide uneven work and propagate worker
  failures;
- Node resolution is growing from exact paths into package exports, conditions, TypeScript paths,
  self-references and alternative install layouts;
- the Tokio migration introduces daemon, watch, filesystem and socket interleavings that ordinary
  timing tests cannot explore;
- `Date`, future `Temporal`, Unicode and `Intl` require large standards and data engines;
- asynchronous HostOps will need capacity, cancellation, permissions and resource authority;
- the realm heap needs exact cyclic collection and a rooting model shared with native execution;
- the compiler query graph will eventually need durability, eviction, cancellation and concurrent
  reads.

These are attractive places to inherit Leverage from the Rust ecosystem. They are also dangerous
places to add dependencies speculatively: a dependency with a broad Interface, types that leak
through the workspace or features Kiln does not use would reduce Locality and iteration speed.

## Decision

### Adoption rule

An ecosystem Module is adopted only when all of the following hold:

1. It sits behind an existing Kiln Seam, or replaces enough repeated local machinery to create a
   genuinely deep Module. One Adapter without a second real use remains a hypothetical Seam.
2. Its types do not enter HIR, IR, protocol records, artifact records, generated-module contracts
   or the public embedding Interface unless a separate ADR explicitly chooses that lock-in.
3. Only required features are enabled. Compiler hot-path crates continue to obey ADR 0004's rebuild
   budget.
4. Any version, ruleset or data bundle that can change observable output participates in the
   relevant query, runtime-package or artifact identity.
5. Deterministic and production Adapters are tested through the same Kiln Interface. A library's
   internal scheduling order is never treated as JavaScript semantics.
6. The replacement passes the deletion test: removing it would force substantial complexity back
   into Kiln rather than merely remove a pass-through wrapper.

### Near-term plan

1. **Rayon owns compiler-side CPU parallelism.** Native compilation, independent lowering and
   conformance work use a bounded Rayon pool rather than growing a Kiln-specific work-stealing
   Implementation. Tokio remains the asynchronous runtime chosen by ADR 0028 and does not execute
   CPU-heavy compiler work; Rayon never schedules realm jobs. Before replacing the current native
   worker, a benchmark must show equivalent results, bounded concurrency, prompt failure
   propagation and no regression on uneven compile inputs.

2. **`oxc_resolver` becomes the rich Node Adapter.** ADR 0027's `ResolutionPolicy` remains the
   Interface and exact-path resolution remains a separate Adapter. After package metadata or an
   equivalent canonical resolution map is pinned into the authoritative source view, the Node
   Adapter should delegate conditional exports, wildcard exports, package imports,
   self-references, TypeScript paths and supported install layouts to `oxc_resolver` rather than
   reimplementing that state machine. The Adapter must resolve against the pinned view used by the
   daemon; it may not silently reread ambient package metadata during graph construction.

3. **Concurrency simulation is test-only infrastructure.** Loom exercises small synchronization
   Modules whose correctness depends on thread interleavings. Turmoil exercises Tokio-hosted
   daemon, socket and filesystem scenarios with seeded schedules and injected failures once those
   paths migrate from blocking threads. Neither is a production event driver, and neither changes
   ADR 0028's runtime choice.

### Feature-triggered plan

4. **Calendar, Unicode and internationalization semantics use linked Rust engines.** When Kiln
   begins `Temporal` or `Intl` coverage, `temporal_rs` and focused ICU4X crates are the default
   candidates under ADR 0025's linked-engine rule. Both interpreted and native execution consume
   the same runtime Implementation. Host time, timezone and locale data remain explicit inputs;
   selected ICU data and engine versions enter the shipped runtime identity. Kiln does not first
   build a second calendar, collation or segmentation engine as a placeholder.

5. **Async HostOps may use Tower and `cap-std` after the operations exist.** Once HostOps has
   multiple asynchronous resources and recurring policy such as readiness, concurrency limits,
   cancellation or permissions, selected Tower layers and capability-bearing `cap-std` resources
   may implement that policy behind the Rust and C Seams. They are not added for the current
   single-operation host. Timeouts and retries are opt-in per operation; a generic retry layer must
   not repeat non-idempotent effects.

### Gated evaluations

6. **Evaluate `gc-arena` before committing the shared realm heap.** ADR 0021's representation,
   frame and safepoint prerequisites remain in force and its collector choice remains open. A
   bounded `gc-arena` spike must prove exact roots across interpreter frames and opaque native
   handles, collection only at realm-safe checkpoints, explicit dynamic roots for pending native
   continuations, no Rust lifetime leakage through C, and acceptable allocation and collection
   cost. Passing the spike makes it the preferred realm-heap Implementation; failing any of those
   conditions leaves ADR 0021's collector sequence unchanged.

   The initial isolated mechanics spike passed cycle collection and `DynamicRootSet` continuation
   rooting with `gc-arena` 0.6.1. That establishes library fit, not the production gate: exact VM
   roots, mutation checkpoints, native frames/opaque handles, teardown and cost evidence remain.

7. **Salsa remains trigger-driven.** ADR 0002's `QueryEngine` Seam and revisit criteria stay
   authoritative. Kiln adopts Salsa when the query graph needs a property such as mixed durability,
   concurrent cancellable reads, cycle recovery or stable persistence that would otherwise become
   the second substantial scaling feature in `MemoEngine`. It is not added merely for ecosystem
   consistency.

### Cranelift is not in the plan

ADR 0009's separately cached generated-C units and portable external toolchain remain the native
backend. Cranelift is not added as a second backend or JIT: it would create a large new lowering and
verification obligation before measurements show that C emission or the external compiler is the
limiting loop. Reconsidering that requires a separate ADR backed by a spike against native build
latency, target coverage, artifact size and conformance.

## Consequences

- The planned production schedulers have disjoint ownership: Tokio owns asynchronous execution and
  realm wakeups; Rayon owns synchronous compiler-side parallel work; the realm owns observable
  JavaScript ordering.
- The Node, internationalization and future host-resource Implementations gain Depth without
  leaking dependency-specific types through compiler consumers.
- Deterministic testing expands beyond virtual time to concurrency and I/O failure schedules while
  adding no alternate production driver.
- Heap and query-engine dependencies remain evidence-gated because their migration costs and
  compile-time effects are materially higher than the near-term candidates.
- Dependency updates that can alter observable semantics are explicit compatibility changes, not
  routine lockfile churn.
- The C backend remains the sole native code-generation path, avoiding a second backend maintenance
  burden.

## Sources

Checked 30 August 2026.

- Rayon parallel iterators, structured tasks and custom pools:
  <https://docs.rs/rayon/latest/rayon/>.
- Oxc Node, ESM, CommonJS and TypeScript resolution:
  <https://docs.rs/oxc_resolver/latest/oxc_resolver/>.
- Loom concurrency-model testing: <https://docs.rs/loom/latest/loom/>.
- Turmoil deterministic Tokio network and filesystem simulation:
  <https://github.com/tokio-rs/turmoil>.
- `temporal_rs` for ECMAScript Temporal implementations:
  <https://docs.rs/temporal_rs/latest/temporal_rs/>.
- ICU4X's selectable internationalization crates and data providers:
  <https://docs.rs/icu/latest/icu/>.
- Tower request policy layers: <https://docs.rs/tower/latest/tower/struct.ServiceBuilder.html>.
- Capability-oriented filesystem and network resources: <https://docs.rs/cap-std/latest/cap_std/>.
- `gc-arena` exact incremental collection within single-threaded arenas:
  <https://github.com/kyren/gc-arena>.
- Salsa durability, backdating, cancellation and memo retention:
  <https://docs.rs/salsa/latest/salsa/>.
