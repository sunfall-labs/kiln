# kiln

kiln is a compiler from JavaScript and TypeScript to native executables, written in Rust.

Its distinguishing bet is stated in [docs/design.md](docs/design.md): iteration speed is a
primary architectural constraint, ranked directly after correctness and above native runtime
performance. The consequence is that the incremental query database, the persistent compiler
process and the artifact cache are foundational rather than later enhancements — incrementality
is built before the compiler gets large, not retrofitted once it hurts. The intended end state
is a tool that feels like an interpreter or a modern dev server while still producing
aggressively optimized native binaries.

## Status: first linked interpreter and native slices work

This repository is at v0. It now has a production Oxc frontend, owned executable HIR, memoized
stack bytecode, an exact-path module graph, a scalar interpreter, deterministic per-module C
emission, a versioned HostOps boundary, and a layered native artifact cache. `kiln run` executes a
deliberately small but real JavaScript/TypeScript subset immediately; `kiln build` and
`kiln run --native` compile that same graph to a native executable through Zig, Clang, `cc`, or
GCC.

What exists today:

| Crate | What it is |
| --- | --- |
| `crates/kiln-hir` | Dependency-free, compiler-owned module and executable HIR. No Oxc type or arena lifetime crosses this boundary. |
| `crates/frontend-oxc` | The Oxc adapter. Parses JS, JSX, TS and TSX and lowers into owned HIR. |
| `crates/kiln-ir` | Parser-independent, versioned stack bytecode and HIR compiler. |
| `crates/kiln-interpreter` | Deterministic scalar VM with bounded execution. |
| `crates/kiln-module-resolver` / `crates/kiln-module-graph` | Project-confined exact-path resolution, deterministic graph construction, link validation, and acyclic execution order. |
| `crates/kiln-host-abi` / `crates/kiln-runtime` | Versioned native-operation contract and the capturing CLI host. |
| `crates/kiln-backend-c` | Deterministic, validated C translation units for the scalar bytecode surface. |
| `crates/kiln-native-number` | Versioned, locale-free Ryu runtime for ECMAScript-exact native number formatting. |
| `crates/kiln-native-runtime` | Cacheable CLI HostOps runtime plus a tiny per-module launcher. |
| `crates/kiln-artifact-cache` | Layered SHA-256 artifact keys, validated records, and atomic writes. |
| `crates/kiln-native-toolchain` / `crates/kiln-native` | Portable compiler/linker boundary and cached native build coordinator. |
| `crates/kiln-db` | Incremental query database. Memoizes parse, bytecode and interface queries with early cutoff. |
| `crates/kiln-test262` / `crates/kiln-conformance` | Test262 metadata discovery and explicit parser/interpreter variant classification. |
| `crates/kiln-test-selector` | Deterministic failures-first, smoke, and affected-test selection. |
| `crates/kiln-proto` | CLI-to-daemon wire contract, at `PROTOCOL_VERSION` 10, including structured interpreter, native-build, native-run, shared-library, and test results. |
| `crates/kiln-daemon` | The long-lived process that owns a database and answers requests over the protocol. |
| `crates/kiln-cli` | The `kiln` binary: a thin client over the daemon. |
| `bench` | The timing harness (`kiln-bench`), which measures the loops §13 wants gated and writes results under `bench/results/`. |

The executable subset currently covers scalar literals, variables, arithmetic, strict equality
and comparisons, assignment, blocks, `if`/`else`, `while`, short-circuit expressions,
conditionals, `console.log`, side-effect imports, and named/default scalar imports and exports in
acyclic graphs. Unsupported valid syntax is rejected with a source range during IR compilation.
Live bindings and cycles, namespace imports, re-exports, functions, objects, exceptions, async
behavior, built-ins, the full Test262 harness, realms, and broad language coverage remain to be
implemented. Native number-to-string conversion now uses shortest ECMAScript spelling;
process-lifetime string allocations remain explicit ownership work rather than conformance
success.

The useful thing that does work is the invalidation behavior. Editing a file's contents
reparses that file; if the edit does not change what the module exports, the project-level query
is not recomputed. `Stats` makes that observable, and the tests and the harness assert on it.
That property is the foundation everything else in the design rests on, which is why it is the
first thing built.

## Priorities

The design document's ordering, which is load-bearing rather than decorative:

1. Correctness
2. Iteration speed
3. Architectural flexibility
4. Native runtime performance
5. Maximum compatibility

The interesting entries are the second and third. Putting iteration speed above native runtime
performance is what justifies an interpreter-first execution path and a daemon that never dies;
putting architectural flexibility above maximum compatibility is what justifies declining a
convenient dependency, or a convenient shortcut through the query graph, in order to keep a
seam. Where those trade-offs have been made concretely, they are in `docs/adr/`.

## The three loops

kiln optimizes three separate edit-to-feedback loops, and they are served by different parts of
the system.

The compiler developer edits Rust, rebuilds and runs tests. This loop is served by workspace
discipline: small foundational crates, a shallow dependency graph, Oxc isolated behind a
frontend crate, and `cargo build --timings` inspected regularly. ADR 0004 has the rules.

The language developer changes semantics or a built-in and runs the affected conformance tests.
This loop is served by keeping built-ins in JS/TS so that changing one does not recompile Rust
(§5), by a stable HostOps ABI (§6), and by test selection that runs what a change can actually
affect (§9). ADR 0005 covers what that buys and what it risks.

The compiler user edits JS/TS and runs the program. This loop is served by the daemon, the query
database and the interpreter-first execution path (§1, §2, §4), with native compilation happening
in the background rather than on the critical path.

All three loops now have a first end-to-end slice. The language-developer loop discovers an
external Test262 tree, prioritizes prior failures and smoke/affected tests, and executes the
selected variants in one resident daemon request. Unsupported semantics remain visible rather
than being counted as passes.

## Running it

Rust 1.97.1 and edition 2024. The toolchain is pinned in `rust-toolchain.toml`, so `rustup` will
fetch the right one on first build. Cargo is enough for interpreter development; native commands
also need Zig, Clang, a compatible `cc`, or GCC on `PATH`.

```sh
cargo build              # build the workspace
cargo test               # the invalidation tests are the interesting ones
cargo run -p kiln-cli -- --one-shot run examples/hello.ts
cargo run -p kiln-cli -- watch examples/hello.ts
cargo run -p kiln-cli -- --one-shot run examples/modules/main.ts
cargo run -p kiln-cli -- --one-shot run examples/hello.ts --native
cargo run -p kiln-cli -- --one-shot build examples/hello.ts --profile native-dev
cargo run -p kiln-cli -- --one-shot test tests/conformance/smoke
cargo run -p kiln-cli -- test /path/to/test262/test --changed
cargo run -p kiln-bench  # the timing harness; results land in bench/results/
cargo build --timings    # the compiler developer's loop, measured (ADR 0004)
```

`kiln watch` runs the entry in the interpreter immediately and after each byte-changing save.
Program output stays on stdout, diagnostics stay on stderr, and a failed run does not end the
watch—the next edit gets another chance.

The daemon speaks the request set in `crates/kiln-proto/src/lib.rs`: `Ping` for liveness and
protocol negotiation, `SetSource` to replace a file's contents, `Check` for one module's
summary, `Run` for interpreted execution, `NativeBuild` / `NativeRun` for cached native work,
`NativeSharedBuild` for embedding libraries, `Test` for resident conformance batches, `Project`
for the whole-project rollup, and `Stats` / `ResetStats` for the recompute counters. A complete
source sync also removes files that disappeared from the client's project view. Test failures and
native artifacts are remembered under `.kiln/`. The exact CLI spelling is in `kiln --help`.

## Design and decisions

[docs/design.md](docs/design.md) is the founding document, kept as written. It is not edited in
place; deviations and refinements are recorded as ADRs beside it, so that a change to the plan
is visible next to the reasoning for it.

- [0001](docs/adr/0001-query-layer-behind-our-own-trait.md) — the query layer sits behind our own
  trait rather than a library's types, so the compiler is not permanently tied to one engine.
- 0002 — what we require of a candidate query-engine backend, such as salsa.
- [0003](docs/adr/0003-interface-hash-is-early-cutoff.md) — §3's interface hash is realized as an
  early-cutoff query rather than a second hash maintained by hand.
- [0004](docs/adr/0004-rust-workspace-discipline.md) — §12's workspace rules restated as
  enforceable rules, with a stated cost for breaking one.
- [0005](docs/adr/0005-builtins-in-js-bootstrap-risk.md) — keeping built-ins in JS/TS, and the
  bootstrap risk that comes with a compiler depending on its own output.
- [0006](docs/adr/0006-owned-hir-and-injected-frontends.md) — Oxc lowers into dependency-free,
  compiler-owned HIR and is injected into the query engine rather than becoming its dependency.
- [0007](docs/adr/0007-interpreter-bytecode-and-hostops-boundary.md) — interpreter bytecode is a
  memoized parser-independent query, while side-effecting execution crosses a versioned HostOps
  boundary and is replayed on every run.
- [0008](docs/adr/0008-test262-is-an-external-explicit-conformance-input.md) — Test262 remains an
  external corpus and every execution variant and unsupported outcome stays explicit.
- [0009](docs/adr/0009-native-c-units-have-layered-content-addressed-artifacts.md) — deterministic
  per-module C, the HostOps runtime, launcher, objects, and executable are separately keyed native
  artifacts behind a portable compiler boundary.
- [0010](docs/adr/0010-hashing-strategy.md) — stable protocol-visible hashes use a specified
  SHA-256 truncation, while cache keys retain the full digest and explicitly encode their inputs.
- [0011](docs/adr/0011-explicit-module-graph-and-acyclic-scalar-linking.md) — module resolution and
  graph traversal are explicit, deterministic layers; the first linkage slice copies acyclic
  scalar exports while reserving live cells and strongly connected components for the next step.

## License

The workspace manifest declares MIT. A `LICENSE` file has not been added yet.
