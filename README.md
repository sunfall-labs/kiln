# kiln

kiln is a compiler from JavaScript and TypeScript to native executables, written in Rust.

Its distinguishing bet is stated in [docs/design.md](docs/design.md): iteration speed is a
primary architectural constraint, ranked directly after correctness and above native runtime
performance. The consequence is that the incremental query database, the persistent compiler
process and the artifact cache are foundational rather than later enhancements — incrementality
is built before the compiler gets large, not retrofitted once it hurts. The intended end state
is a tool that feels like an interpreter or a modern dev server while still producing
aggressively optimized native binaries.

## Status: kiln does not compile anything yet

This repository is at v0, which is the substrate the design says to build first (§14, items one
through three). It contains a query engine, a daemon, a CLI and a timing harness. It does not
contain a compiler.

What exists today:

| Crate | What it is |
| --- | --- |
| `crates/kiln-db` | The incremental query database. Defines the `QueryEngine` trait and the values the compiler exchanges with it, and ships `MemoEngine`, a hand-written memoizing implementation with early cutoff. Zero dependencies, deliberately. |
| `crates/kiln-proto` | The CLI-to-daemon wire contract, at `PROTOCOL_VERSION` 1. Request and response types plus the `Stats` counters. |
| `crates/kiln-daemon` | The long-lived process that owns a database and answers requests over the protocol. |
| `crates/kiln-cli` | The `kiln` binary: a thin client over the daemon. |
| `bench` | The timing harness (`kiln-bench`), which measures the loops §13 wants gated and writes results under `bench/results/`. |

What does not exist: there is no parser, no HIR, no IR, no interpreter, no C backend, no linker
integration, no runtime library, no HostOps ABI, no built-ins, no test selection engine and no
Test262 runner. Nothing here turns JavaScript into a program. The `Parsed` value that the engine
caches is a stand-in shaped like a parse result, present so that the incremental machinery can
be built and tested against the shape it will eventually carry: its export list comes from a
line-oriented scan that knows nothing about strings, comments or multi-line declarations, and it
is not a parser in any sense. The Oxc frontend is §14 item four and has not started.

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

Today only the first loop is genuinely served, because only the first loop exists.

## Running it

Rust 1.97.1 and edition 2024. The toolchain is pinned in `rust-toolchain.toml`, so `rustup` will
fetch the right one on first build. Cargo is the only prerequisite.

```sh
cargo build              # build the workspace
cargo test               # the invalidation tests are the interesting ones
cargo run -p kiln-cli    # the `kiln` binary
cargo run -p kiln-bench  # the timing harness; results land in bench/results/
cargo build --timings    # the compiler developer's loop, measured (ADR 0004)
```

The daemon speaks the request set in `crates/kiln-proto/src/lib.rs`: `Ping` for liveness and
protocol negotiation, `SetSource` to replace a file's contents, `Check` for one module's
summary, `Project` for the whole-project rollup, and `Stats` / `ResetStats` for the recompute
counters. The exact CLI spelling of each is in `kiln --help`; the wire types are the contract.

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

## License

The workspace manifest declares MIT. A `LICENSE` file has not been added yet.
