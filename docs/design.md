# kiln design document

This is kiln's founding document, reproduced as written. Everything the project does is
downstream of it: the priority order in the first section is what makes the query database, the
daemon and the timing harness foundational rather than optional, and the implementation order in
§14 is the plan of record.

It is not edited in place. Where the implementation departs from what is written here, the
departure is argued in an ADR under `docs/adr/` and this file stays as it was, so that the
reasoning behind a change is visible next to the thing it changed rather than folded silently
into the original. ADR 0003 is the first such deviation: it keeps §3's guarantee that dependents
are recompiled only when an interface changes, but obtains it from the query engine instead of
from a second hash maintained by hand.

Section numbers are stable and are cited from the code (`design.md §7`, and so on). Treat them
as identifiers, not as incidental numbering. The document opens mid-conversation because it was
written as a revision of an earlier plan; that opening is kept rather than tidied away, because
the thing being revised is the whole point.

---

Agreed. Iteration speed should be a primary architectural constraint alongside runtime performance and compatibility.

The project should optimize three separate loops:

1. Compiler developer: edit Rust → rebuild → run tests.
2. Language developer: change semantics/built-ins → run affected conformance tests.
3. Compiler user: edit JS/TS → run interpreted or native program.

## Revised priority order

```text
1. Correctness
2. Iteration speed
3. Architectural flexibility
4. Native runtime performance
5. Maximum compatibility
```

This changes several design decisions: the interpreter, incremental compiler database, persistent process and artifact cache become foundational—not later enhancements.

## 1. Persistent compiler service

The CLI should be a thin client over a long-running compiler process:

```text
CLI/watch/editor
       ↓
compiler daemon
├── source database
├── module graph
├── parsed/HIR cache
├── semantic cache
├── IR cache
├── generated-C cache
└── object cache
```

Commands such as:

```sh
compiler run app.ts
compiler watch app.ts
compiler test --changed
```

reuse the same warm process. This avoids repeatedly loading Oxc, dependencies, built-in definitions and runtime metadata.

A standalone one-shot mode remains available for CI and reproducible builds.

## 2. Query-based incremental compilation

Model compilation as cached queries:

```text
source(file)
    ↓
parse(file)
    ↓
lower_hir(file)
    ↓
analyze_module(file)
    ↓
build_ir(function)
    ↓
optimize_ir(function, profile)
    ↓
emit_c(module)
    ↓
compile_object(module, target)
```

Each result is keyed by:

- Source hash
- Compiler revision
- Target
- Optimization profile
- Relevant dependency-interface hashes
- Runtime ABI version

When one file changes, only dependent queries are invalidated.

Salsa is worth evaluating for this because it memoizes query results and recomputes only affected values, although its repository still describes it as work in progress. We should hide it behind our own query interface so the compiler is not permanently tied to it. [Salsa's query model](https://github.com/salsa-rs/salsa) fits the design well.

## 3. Module-level incrementality first

Start with module-level invalidation:

```text
changed source module
        ↓
reparse module
        ↓
reanalyze module
        ↓
regenerate one C translation unit
        ↓
compile one object
        ↓
relink
```

Add function-level incrementality only after measuring whether module granularity is insufficient. One C file per function could create excessive filesystem and linker overhead.

Every module should have two hashes:

```text
implementation hash
    Changes whenever internal code changes.

interface hash
    Changes only when exports or externally relevant semantics change.
```

Dependents are recompiled only when the interface hash changes.

## 4. Interpreter-first development

The fastest edit-to-run path should not invoke `zig cc`:

```text
edit source
    ↓
incremental parse/lower
    ↓
execute cached IR
```

Target warm development latency:

- Small source edit to execution: under 100–200 ms
- Medium project edit to execution: under 500 ms
- Targeted semantic test: under one second

These are initial design goals to calibrate on a reference machine.

Native compilation can happen in the background:

```text
foreground: interpret immediately
background: update native objects and report backend mismatch
```

This means developers get instant results while native-backend coverage remains active.

## 5. Built-ins remain JS/TS in development

Implement as much runtime behavior as possible in JS/TS:

- Node argument validation
- Error construction
- Streams
- EventEmitter
- Buffer facade
- Filesystem facade
- Promise/callback conversion
- Utility algorithms

Then changing a built-in does not require recompiling Rust:

```text
edit node:fs wrapper
    ↓
compiler incrementally recompiles JS
    ↓
run targeted tests
```

Rust should contain only functionality that genuinely needs native access or performance.

In development mode, built-ins can execute as cached IR bytecode. Release builds compile the same IR to C.

## 6. Stable native-operation ABI

HostOps should evolve additively and remain stable across ordinary compiler changes.

```text
JS built-ins
    ↓
stable HostOps ABI
    ↓
Rust runtime/provider
```

Benefits:

- Application objects remain reusable after runtime implementation changes.
- Built-ins remain reusable after compiler CLI changes.
- The runtime can be a dynamic library during development.
- Release builds can link the same runtime statically.

Development:

```text
application bytecode/objects
libjsruntime.dylib
```

Release:

```text
application objects
libjsruntime.a
    ↓
single executable
```

If the ABI does not change, editing the runtime only rebuilds the runtime library.

## 7. Layered artifact cache

Use a content-addressed cache for:

```text
source → HIR
HIR → semantic result
HIR → IR
IR → optimized IR
IR → generated C
generated C → object
objects → executable
```

An artifact is reused whenever its inputs have identical hashes, regardless of path or build session.

Cache namespaces should include:

- Compiler version
- Runtime ABI
- Target triple
- CPU features
- Debug/release profile
- Node compatibility version

Built-in and npm dependency artifacts should be highly durable because they change far less often than application code.

## 8. Fast native builds

When native execution is needed:

- Emit one C translation unit per module.
- Compile only changed units with `zig cc -c -O0`.
- Reuse cached object files.
- Precompile runtime and Node compatibility libraries.
- Link through Zig/LLD.
- Avoid static runtime rebuilding in development.
- Generate compact, deterministic C without expensive formatting.
- Compile independent translation units in parallel.

Build profiles:

```text
interpret
    No C compiler.

native-dev
    Cached objects, dynamic runtime, -O0, fast link.

native-check
    Cached objects, sanitizers and assertions.

native-release
    Static runtime, optimized C, optional LTO.
```

## 9. Test selection engine

Each test should declare or infer what it exercises:

```yaml
test: closures/capture-let.js
features:
  - lexical-environments
  - closures
ir:
  - make_environment
  - load_capture
  - store_capture
```

When code changes, select affected tests from:

- Modified compiler crate
- Modified IR operations
- Modified built-in module
- Modified HostOps operation
- Previously failing tests
- A small universal smoke suite

Local test tiers:

```text
Tier 0: unit/smoke             < 1 second
Tier 1: affected regressions   < 3 seconds
Tier 2: affected Test262/Node  < 10 seconds
Tier 3: native equivalence     background
Tier 4: complete suites        CI
```

Failed tests run first on the next attempt.

## 10. Persistent conformance workers

Do not launch a fresh process for every Test262 or Node test.

Use persistent workers:

```text
test coordinator
├── interpreter worker
├── interpreter worker
├── native shard worker
└── native shard worker
```

Each worker:

- Loads the runtime once.
- Loads common harness code once.
- Creates a fresh realm/test context.
- Executes many tests.
- Resets state between tests.

The first implemented worker slice reuses only the deterministic current-thread scheduler on each
resident Rayon thread. Every interpreter case still constructs and reclaims a fresh VM, language
realm, global object, host capture, timer queue, and value graph. A panic retires that scheduler
before the thread accepts another case. Parse-only work needs no runtime, and Node workers remain a
separate slice until their sockets, HTTP Agent, module session, and provider resources have an
explicit reset contract; treating terminal realm shutdown as a per-test reset would be incorrect.
The shared Test262 harness is already discovered once and retained with the resident corpus.

Tests requiring process isolation still receive separate processes.

Native conformance tests can be compiled into shards rather than one executable per test:

```text
test262-array-001 executable
    contains 500 independent cases
```

A failing test can then be reproduced as a standalone artifact.

## 11. Deterministic event-loop tests

The mock event driver should use virtual time:

```javascript
const timer = setTimeout(callback, 1000);
testRuntime.advanceTime(1000);
```

This makes async tests fast and deterministic:

- No real sleeping
- No filesystem/network latency
- Exact callback-order assertions
- Repeatable race tests
- Easy failure minimization

Real libuv integration tests remain a smaller separate suite.

## 12. Rust workspace discipline

Rust rebuild time must be monitored continuously.

Design rules:

- Keep foundational type crates small and stable.
- Avoid changing central generic types frequently.
- Keep Oxc isolated behind `frontend-oxc`.
- Separate interfaces from implementations.
- Minimize procedural macros in frequently edited crates.
- Avoid expensive build scripts.
- Disable unnecessary dependency features.
- Keep the crate dependency graph shallow.
- Put experimental optimizers in leaf crates.
- Avoid forcing the runtime to rebuild when the compiler changes.

Use Cargo's incremental compilation and regularly inspect `cargo build --timings`; Cargo's timing reports show the critical path, code-generation time and concurrency. [Cargo documents the facility here](https://doc.rust-lang.org/cargo/reference/timings.html).

## 13. Performance regression gates

CI should record:

- Clean Rust build
- Incremental Rust build after leaf edit
- Incremental Rust build after IR edit
- Compiler daemon startup
- Warm parse/lower latency
- Warm interpreted execution latency
- Warm native object rebuild
- Link latency
- Targeted Test262 latency
- Full Test262 throughput
- Generated C size
- Cache hit rate

Initial goals on a fixed reference machine:

| Operation | Goal |
|---|---:|
| No-op compiler rebuild | under 1 s |
| Typical leaf-crate rebuild | under 3 s |
| Warm small JS edit → interpreter | under 200 ms |
| Warm medium edit → interpreter | under 500 ms |
| Warm small native rebuild | under 1 s |
| Affected regression tests | under 3 s |
| Affected conformance tests | under 10 s |

These should be calibrated after the initial spike, but regressions should require explicit justification.

## 14. Updated implementation order

The revised sequence becomes:

1. Timing harness and reference workloads
2. Persistent compiler process
3. Incremental source/query database
4. Oxc → HIR
5. Minimal IR interpreter
6. Changed-test selection
7. Test262 parser/interpreter runner
8. Cached multi-unit C backend
9. Rust runtime with stable HostOps ABI
10. Node compatibility graft
11. Async event loop and deterministic driver
12. Native optimization
13. Release packaging

The important change is that incrementality is built before the compiler becomes large.

## 15. Primary development workflow

The intended everyday loop:

```sh
compiler watch app.ts
```

On every edit:

```text
1. Detect changed module.
2. Recompute only invalidated compiler queries.
3. Run affected regressions in the interpreter.
4. Launch the application immediately.
5. Rebuild changed native object in the background.
6. For successful scalar results, compare byte-exact stdout and exact result identity between
   interpreter and native execution; grow this into the canonical intermediate-state oracle as
   object/native support lands.
7. Report conformance or performance regressions.
```

That should make the system feel closer to an interpreter or modern dev server during development, while still producing aggressively optimized native executables for release.
