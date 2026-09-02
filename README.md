# kiln

kiln compiles JavaScript and TypeScript to native executables. It is written in Rust, emits C,
and links the result against a small Rust runtime built on Tokio and Hyper. There is no
JavaScript engine inside the binary.

A ten-line `node:http` server compiles to a 3.8 MB executable that starts in 8 ms, holds
about 30 MB resident under load, and serves requests at the same rate as Node, Bun and Deno on
the same machine. The interpreter that ships alongside the compiler runs the same program
immediately, so the edit-run loop never waits for a native build.

## Status

kiln is at v0 and under active development. Today it runs and compiles a real but bounded subset
of the language and of Node's API: modules (ESM and CommonJS), classes, closures, generators,
async functions and promises, the standard built-ins including `Map`, `Set`, typed arrays, JSON,
regular expressions, BigInt and Temporal, and the `node:http`, `node:net`, `node:fs`,
`node:events`, `node:stream`, `node:buffer`, `node:path` and `node:url` surfaces that small
services use. What is not implemented is refused with a source location rather than
approximated; `docs/language-coverage.md` records the ladder.

Conformance is measured, not asserted. The native backend is held to byte-identical output with
the interpreter on every probe in `conformance/native`, and the interpreter is held to Test262.

## Measured

`bench/server`, 2026-09-01, one machine, keep-alive, median of three interleaved passes.

A hello-world server:

| runtime | 1 connection | 16 connections | 64 connections | resident |
|---|---|---|---|---|
| kiln native | 25.5k rps | 71.2k rps | 67.3k rps | 27 MB |
| node 25.9 | 27.2k rps | 78.2k rps | 70.7k rps | 83 MB |
| bun 1.4 | 23.7k rps | 64.8k rps | 67.9k rps | 67 MB |
| deno 2.9 | 25.9k rps | 80.8k rps | 74.3k rps | 108 MB |

A small service (URL routing, query parsing, JSON in and out, a POST body, an 8 KB response,
header echo, an async handler), every answer checked by the client:

| runtime | 16 connections | 64 connections | resident |
|---|---|---|---|
| kiln native | 68.6k rps | 62.2k rps | 35 MB |
| node 25.9 | 70.8k rps | 41.9k rps | 96 MB |
| bun 1.4 | 52.7k rps | 58.2k rps | 85 MB |
| deno 2.9 | 46.7k rps | 59.9k rps | 120 MB |

The machine was shared while these ran; the ranges are in
`.scratch/server-throughput/measurements-2026-09-01.md`. `bench/server/run.sh` reproduces a row.

## Using it

Rust 1.97.1 (pinned in `rust-toolchain.toml`) and a C compiler: Zig, Clang, cc or GCC on `PATH`.

```sh
cargo build
cargo run -p kiln-cli -- --one-shot run examples/hello.ts                    # interpret
cargo run -p kiln-cli -- --one-shot run examples/hello.ts --native           # compile and run
cargo run -p kiln-cli -- --one-shot build server.mjs --node --profile native-release --output server
./server
```

`--node` selects Node's module resolution and installs the Tokio-owned adapter for `node:*`.
`kiln watch` re-runs an entry in the interpreter on every save. `kiln test <dir>` runs a
conformance corpus; the corpora and the runner each one needs are listed in `AGENTS.md`.

## How it is built

The priority order in `docs/design.md` is correctness, iteration speed, architectural
flexibility, native performance, compatibility, in that order, and the shape of the repository
follows from it.

- **An incremental query database is the foundation, not a later optimisation.** Parsing,
  lowering and interface hashing are memoised queries with early cutoff; editing a file that does
  not change what it exports recomputes nothing downstream. `kiln-db` is the smallest crate and
  the most load-bearing.
- **One owned intermediate representation.** Oxc parses; `kiln-hir` is dependency-free and no
  parser type crosses it. The interpreter and the C backend consume the same versioned stack
  bytecode from `kiln-ir`, which is what makes byte-identical differential testing possible.
- **One runtime for interpreted and native code.** A realm is one local task on a current-thread
  Tokio runtime. Promise jobs, timers and HTTP events are ordered by the realm's own queues and
  sequence numbers; Tokio schedules, the realm decides. Generated C reaches the runtime through a
  private, versioned C ABI of opaque handles.
- **Hyper owns HTTP; the realm owns Node semantics.** A server connection is one Hyper task over
  the accepted socket, and protocol events call the compiled handler directly.
- **The native heap is collected at the turn boundary.** A precise mark-sweep collector runs
  between callbacks; every allocation a value can point at is a tagged block, and marking never
  trusts a pointer it cannot look up.

Every decision of that kind is an ADR under `docs/adr/`, numbered and never edited in place;
`docs/design.md` is the founding document, kept as written.

## Layout

| crate | role |
|---|---|
| `kiln-hir`, `frontend-oxc`, `kiln-ir` | owned HIR, the Oxc adapter, and the bytecode compiler |
| `kiln-db` | the incremental query database |
| `kiln-module-resolver`, `kiln-module-graph` | exact-path resolution and the module graph |
| `kiln-interpreter` | the deterministic bytecode VM |
| `kiln-backend-c` | the C emitter and the C runtime prelude |
| `kiln-runtime`, `kiln-host-abi` | the Tokio/Hyper realm runtime and the host-operation contract |
| `kiln-native-number-ffi`, `kiln-native`, `kiln-native-toolchain` | the runtime archive linked into executables, the native build coordinator, the compiler boundary |
| `kiln-daemon`, `kiln-cli` | the resident compiler process and the `kiln` binary |
| `kiln-conformance`, `kiln-test262`, `kiln-test-selector` | corpus discovery, classification and failures-first selection |
| `bench` | the timing harness and the server benchmarks |

## Contributing

`AGENTS.md` holds the working rules for this repository: which runner each corpus needs, why
measurements are taken in a private worktree, how the native runtime archive is built, and what
to do when the disk fills. `NOTES.md` is the dated log of what was tried and what it cost.

## License

MIT. See `LICENSE`.
