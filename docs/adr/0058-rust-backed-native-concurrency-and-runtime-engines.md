# ADR 0058: Runtime engines are Rust, linked across a coarse-grained provider ABI

## Status

Proposed. This revision (2026-09-06) replaces the same-day draft, which cited a superseded ADR,
a stale ABI version, two methods that do not exist and a vectorization consequence the emitter
cannot deliver. The rule is the same; the claims are now the ones the code supports.

Extends ADR 0025 (language semantics that need an engine are linked, not reimplemented), ADR 0028
(one Tokio runtime owns interpreted and native execution) and ADR 0029 (mature Rust
implementations at existing seams). Refines ADR 0057 §4, whose `parallelMap` lowering landed as a
sequential loop. ADR 0018 is superseded by 0028 and is not a basis for anything here.

## Context

The compiled backend emits C99 translation units linked against one Rust static library, the
provider archive (`crates/kiln-native-number-ffi`, 87 exports, `PROVIDER_ABI_VERSION` 30). The
archive already carries the number grammar, `regress`, BigInt, Temporal, `node:path` and the Tokio
realm runtime. It is a separate build from the workspace rlib
(`scripts/package-native-number-provider.sh`), and the packager verifies its symbol table against
`PROVIDER_EXPORTS`.

Two facts about the C side bound what any Rust engine can be asked to do:

* **A closure body runs on the realm thread only.** Allocation resolves the heap through a
  `_Thread_local` realm cache (`runtime/heap.c`), `kiln_c99_inheriting_new_target` is a process
  global, and the turn-boundary collector (ADR 0041) assumes no concurrent mutator. A closure that
  allocates, or reads any heap object other than its input, cannot execute on a foreign thread.
* **Nothing crosses the boundary per element.** ADR 0025's regexp seam is the model: an opaque
  handle, scalars and byte offsets, one call per operation. A callback per element is the shape
  that costs `postcss` weeks in `undefined` (CLAUDE.md) and it destroys any vectorization on
  either side.

Two places currently violate the principle this ADR states:

1. **`JSON.parse` exists twice.** A recursive-descent reader in `runtime/json.c` (line 513
   onward) and a `JsonReader` in `crates/kiln-interpreter` implement the same grammar, and they
   have already disagreed once: `conformance/differential/json-surrogates.js` records the
   interpreter decoding each half of a surrogate pair on its own while the native backend got it
   right. The hot part of a parse of a small document is object construction (`kiln_c99_put`,
   shape transitions, key interning), which must stay in C; the grammar, unescaping, surrogate
   handling and number conversion need not.
2. **`parallelMap` is a sequential `map`.** Commit 555de594 lowered it in
   `emit_graph/ops/call_method.rs` as a single-threaded loop that calls `plain->body` through a
   function pointer for every non-generator closure. The `is_pure()` predicate added to
   `graph.rs` is called only by a test, and checks for stores, not allocation. The `.d.ts` says
   `Promise<U[]>`, the JS polyfill returns a promise, the native lowering returns an array, and
   the native probe polyfills to `Array.prototype.map`, so no corpus exercises parallelism. The
   emitter has no closure inlining, and array elements are NaN-boxed `kiln_val`, not `double`, so
   nothing auto-vectorizes.

`rayon` is already a workspace dependency and `crates/kiln-native/src/worker.rs` already runs
compile jobs on it.

## Decision

**An engine or a nontrivial algorithm is implemented once, in Rust, in the provider archive, and
both engines use it.** The interpreter calls the Rust function directly through the rlib; emitted
C calls it across the provider ABI. The boundary carries scalars, byte ranges and opaque handles
only, one crossing per operation, never per element. Emitted C keeps exactly the part that must
touch the C heap: allocating objects, arrays and strings, and inserting properties. Grammar,
decoding, numeric conversion, scheduling and algorithms are Rust.

The condition is that nothing observable is given up: key order, error positions, number edge
cases, the surrogate replacement ADR 0022 records, and the refusal-over-`undefined` discipline
all hold on the Rust path or the change does not land.

### Application 1: `JSON.parse` over a Rust reader (landed, c963d4bc)

The archive gains a `json` feature backed by a new crate, `kiln-json`, whose reader is a one-pass
scalar scanner producing a flat tape: a node array plus an arena of already-decoded UTF-8.
The interpreter links the crate directly; emitted C calls one export, `kiln_json_parse`, which
lends a per-thread tape, and walks it. Both the C recursive-descent reader and the interpreter's
`JsonReader` are deleted.

* Rust owns the grammar, unescaping, surrogate-pair combination, lone-surrogate replacement
  with U+FFFD, `-0`, the exact-multiply number fast path with `f64::from_str` as the
  correctly-rounded fallback, and every SyntaxError. C keeps only allocation, string
  materialisation, and property definition through the direct shape-transition path main's
  reader introduced in 02792a3d, with keys interned straight from the arena.
* **simd-json was the first reader and was measured out.** Its DOM loses key order, so only
  its tape was ever usable, and on documents of the benchmark's shape a scalar pass beat it at
  every size from 159 bytes to 8.5 MB (1.0x to 1.27x), because its two fixed SIMD stages over
  padded 64-byte blocks cost more than one scalar pass, and its tape had to be converted into
  ours. The crate has no dependencies. The table is in `crates/kiln-json/src/lib.rs` and in
  `.scratch/json-tape/NOTES.md`, with two slower scanner variants recorded so they are not retried.
* **Parity was reached and measured before landing**, 20 interleaved rounds against the base
  commit, best / p25 / median: small payload 0.386 / 0.401 / 0.405 us on main against
  0.377 / 0.398 / 0.402 us; 2.2 MB parse 5.45 / 6.45 / 6.56 ms against 5.79 / 6.26 / 6.34 ms;
  the interpreter better on both. Best-of-N is dominated by tails on a loaded machine and is not
  the statistic to gate on; median and p25 are.
* Two things found on the way, both filed under `.scratch/json-tape/issues/`: keys with an
  embedded NUL truncate in the shape store on both readers, because interned keys are C strings;
  and commit 9d96e4a8's chunked nursery walks its chunk list on every pointer lookup, which makes
  main's large parse fifty times slower on the native backend regardless of reader.

### Application 2: `parallelMap` over a Rayon fork-join

* **Contract.** `parallelMap` returns an array synchronously. The promise-returning type in
  `packages/kiln-concurrency/index.d.ts` and the `Promise.all` polyfill are wrong and are
  corrected. A blocking fork-join on the realm thread is the simplest design that is correct
  under ADR 0028's one-realm-one-thread rule.
* **Gate.** The backend lowers to the parallel path only for a closure it can prove allocates
  nothing and touches no heap object other than the input element: numeric arithmetic over a
  numeric array, no upvalue reads of heap values, no calls except to builtins on a fixed
  allowlist. `is_pure()` is not that predicate and is replaced by one that is. Anything else is
  refused with the reason named, following the `conformance/native/*-refuses.ts` convention, not
  silently run sequentially.
* **ABI.** One export, `kiln_parallel_map(elements, count, chunk_fn, ctx, out)`, where
  `chunk_fn` is an emitted static C function over a `[start, end)` range. Rust owns the pool, the
  chunk size and a sequential floor below which it runs the chunk inline: a fork-join over five
  thousand multiply-adds is slower than the loop, and `bench/non-io/multicore-parallel.mjs` as
  written would regress.
* **Pool.** A private `rayon::ThreadPool` sized from `available_parallelism`, overridable the
  way `KILN_WORKERS` is. The "shared CPU budget" in `AGENTS.md` governs `mbx` on a developer
  machine; it is not a property of a shipped binary and the earlier draft was wrong to cite it.
* **Not claimed.** Auto-vectorization of the chunk body needs closure inlining in the emitter
  and a `double[]` representation for numeric arrays; neither exists, and both are separate
  ADRs. `parallelReduce` and `parallelSort` are not defined anywhere and are not part of this
  decision.

## Consequences

* **Deleted.** Two JSON readers (~400 lines of C, ~200 of Rust). `crates/kiln-runtime/src/concurrency.rs`
  (394 lines, re-exported and used by nothing). The unwired `is_pure()`. The polyfill hooks
  `__kiln_actor_dispatch`, `__kiln_transfer_region`, `__kiln_parallel_map` and
  `__kiln_schedule_task`, which no engine implements, are removed rather than left as promises.
* **Added.** A `json` feature and a small set of exports in the archive, each with a row in the
  registry and a packager check; a C tape walker; a Rayon fork-join export and a refusing
  predicate in the backend.
* **Measured, not asserted.** Each application lands with timings on both engines and a profile,
  in the tracker under `.scratch/`, with the shape CLAUDE.md's benchmarking section requires.
* **Tracker corrected.** The multicore PRD marked all six issues done on 2026-09-06; the runtime
  delivers none of ADR 0057's three tiers. Those statuses are reset to what landed.

## Alternatives rejected

* **A callback per element or per node.** O(N) FFI crossings, and the shape that produces
  `undefined` from a read that should have refused.
* **simd-json's DOM `Value`.** Loses key order. Unusable regardless of speed.
* **A thread pool in the C prelude.** Contradicts the principle and duplicates Rayon, which is
  already linked.
* **Keeping both JSON readers and adding vectors.** Differential vectors catch what they cover;
  the surrogate bug is what they did not.
