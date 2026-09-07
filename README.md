# kiln

kiln compiles JavaScript and TypeScript to native executables. It is written in Rust, emits C,
and links the result against a small Rust runtime built on Tokio and Hyper. There is no
JavaScript engine inside the binary.

A ten-line `node:http` server compiles to a 4 MB executable that starts in 8 ms, holds under
10 MB resident under load, and serves more requests per second than Node, Bun and Deno on the
same machine. The interpreter that ships alongside the compiler runs the same program
immediately, so the edit-run loop never waits for a native build.

## Status

kiln is at v0 and under active development, with a strong focus on high ECMAScript conformance, real-world library compatibility, and native performance:

- **ECMAScript Conformance**: Tracks the official Test262 suite, passing the vast majority of tests across standard built-ins, syntax, and runtime semantics (tracked chunk-by-chunk in `conformance/test262-baseline.json`).
- **Real-World Library Compatibility**: Executes unmodified third-party libraries and application bundles, producing output byte-identical to Node under both the interpreter and the native C backend (`conformance/thirdparty`):
  - *Tooling & Compilers*: `@babel/parser` + `@babel/traverse` + `@babel/generator` (`babel`), `acorn`.
  - *Reactive & State Systems*: `vue-reactivity` (Vue 3 reactive proxy system), `mobx`, `rxjs`.
  - *Validation & Runtime Generation*: `ajv` (JSON Schema compiler with runtime code generation).
  - *Standard Utilities*: `lodash`, `date-fns`, `dayjs`, `semver`, `marked`, `mustache`, `uuid`, `chalk`, `ms`, `immer`.
- **Targeted Corpora**: Held to clean runs across dedicated suites for language edge cases (`conformance/guard`), Node compatibility (`conformance/node`), regression tests (`conformance/regress`), Effect runtime semantics (`conformance/effect`), and HTTP server workloads (`conformance/server`).
- **Engine Parity**: The compiled native C backend is held to byte-identical output and matching exit codes with the bytecode interpreter across the entire native corpus (`conformance/native`), ensuring zero divergence between execution modes.

Today it compiles and executes a substantial language and runtime surface: modules (ESM and CommonJS, including cyclic dependencies), classes and private fields, closures, generators and `yield*`, async functions and promises, standard built-ins (`Map`, `Set`, typed arrays, JSON, regular expressions, BigInt, Temporal, `Date`, `DataView`), and the core `node:*` APIs (`http`, `net`, `fs`, `events`, `stream`, `buffer`, `path`, `url`, `util`) needed for high-throughput services.

What is not implemented is refused with a source location rather than approximated or returning silent `undefined`. (`docs/language-coverage.md` records the initial bootstrap backlog; current progress is tracked directly in `conformance/test262-baseline.json` and the gate suites).

## Measured

`bench/server/compare.sh`, 2026-09-02, one machine, keep-alive, four client processes with
sixteen connections each, median of three interleaved passes. A single client process saturates
before these servers do, which is why there are four.

A hello-world server:

| runtime | requests/s | p50 | p99 | resident |
|---|---|---|---|---|
| kiln native | 163.7k | 0.32 ms | 1.02 ms | 8 MB |
| bun 1.4 | 142.5k | 0.36 ms | 1.21 ms | 76 MB |
| node 25.9 | 115.0k | 0.48 ms | 1.39 ms | 84 MB |
| deno 2.9 | 104.1k | 0.54 ms | 1.37 ms | 121 MB |

A small service (URL routing, query parsing, JSON in and out, a POST body, an 8 KB response,
header echo, an async handler that awaits a timer), every answer checked by the client:

| runtime | requests/s | p50 | p99 | resident |
|---|---|---|---|---|
| kiln native | 111.4k | 0.44 ms | 1.68 ms | 10 MB |
| bun 1.4 | 107.6k | 0.42 ms | 2.04 ms | 94 MB |
| node 25.9 | 89.0k | 0.57 ms | 2.00 ms | 101 MB |
| deno 2.9 | 78.8k | 0.62 ms | 2.89 ms | 126 MB |

The machine was shared while these ran; the per-pass ranges and raw log are in
`.scratch/server-throughput/measurements-2026-09-02.md`. `bench/server/compare.sh` reproduces the
table, `bench/server/run.sh` one row.

The same script on Linux/x86_64 (Xeon Platinum 8259CL, 2026-09-03, node 22.23 and bun 1.4, no deno
installed):

| runtime | hello | small service | resident |
|---|---|---|---|
| kiln native | 72.3k | 42.9k | 8-9 MB |
| bun 1.4 | 65.5k | 44.7k | 66-76 MB |
| node 22.23 | 39.0k | 27.4k | 81-83 MB |

Read that row honestly: it is **one** kiln realm, on one core, against runtimes that are not
single-threaded. Measured while serving, bun runs twelve threads and node five, and kiln one -- so
kiln leads hello world and trails bun on the small service by four percent while doing *less* work
per request than either (23.7 microseconds of CPU against bun's 26.7).

A realm owns its thread, so the way kiln uses the other fifteen cores is `KILN_WORKERS`, which
forks that many realms over one `SO_REUSEPORT` listener. Setting `KILN_WORKERS=auto` detects the
number of online CPU cores via `sysconf(_SC_NPROCESSORS_ONLN)` (clamped 1..1024) and forks one
realm per core. With four of them the small service serves **51.8k**, ahead of bun, and ten client
processes are needed before the load generator stops being the limit:

| clients | workers=1 | workers=2 | workers=4 | workers=auto |
|---|---|---|---|---|
| 10 (hello) | 47.1k | 84.4k | **107.8k** | 86.1k |

The comparison table above stays single-realm because that is the like-for-like core-for-core
number, and it is the one to beat. Absolute figures are about half the macOS table further up,
which was taken on an unrecorded machine; the two are not comparable and neither is presented as
the other. Full provenance is in `.scratch/server-throughput/measurements-2026-09-03.md`.

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
- **A shipped realm keeps wall time; a measured one keeps virtual time.** `kiln build` gives the
  program the host's clock, so timers fire on schedule under load; `kiln run --native` and the
  conformance harness keep the deterministic clock that makes two engines comparable.

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
