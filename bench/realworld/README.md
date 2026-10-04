# Real-world suite

Four TypeScript programs, every runtime, one table. The microbenchmarks under
`bench/` isolate warm steady state; this suite measures the opposite end: whole
programs as users run them, startup included, with stdout checksummed against
node so a fast wrong answer reads as a divergence, never as a win.

## Programs

| program | tier | what it is | needs |
| --- | --- | --- | --- |
| `programs/json-transform.ts` | A (portable) | 5,000-row synthetic user table: per-city aggregates, top-5 oldest | nothing |
| `programs/template-render.ts` | A (portable) | 2,000-product HTML render with escaping + FNV-1a digest | nothing |
| `programs/file-cli.ts` | B (node) | reads `inputs/users.json`, validates, per-city stats | `node:fs` |
| `programs/http-server.ts` | B (server) | `/hello` text + `/sum?a=&b=` JSON on port 18091 | `node:http` |

Tier A runs everywhere a runtime can execute the language subset; tier B needs
the Node adapter, so porffor and Hermes report it as unsupported (a capability
gate, not a failure). Kiln's native column builds all four programs as
`native-release` AOT executables; tier B passes `--node` at build time to include
the filesystem or HTTP adapter. The Bun column runs the `.ts` source directly.
`inputs/users.json` is committed; regenerate it with `node inputs/gen-users.mjs`.

CLI timings include program startup and exclude AOT builds. Memory is the maximum
of each runtime's per-process peak RSS across repetitions, excluding compilers
and earlier runs. A fresh measurement worker isolates the child resource counter;
the worker's startup and result transport are outside the reported program time.

## Runtimes

`node`, `bun`, `kiln` (interpreter), `kiln-native`, plus, when their
toolchains are present, `porffor` (`porf`) and Static Hermes (`hermesc` + `hvm`
from the `static_h` branch). Anything missing is skipped with the reason
printed, never failed.

## Run

```sh
./bench/realworld/fetch-competitors.sh   # once per machine (porffor fast, hermes slow)
./bench/realworld/compare.sh [out.json]  # PASSES=3 SERVER_REQUESTS=2000
```

`PORF_BIN`, `HERMESC_BIN`, `HVM_BIN`, `KILN_BIN` override toolchain discovery;
`mise run realworld` is the front door. The `.toolchains/` directory is
gitignored; the Hermes checkout lives in `~/.cache/kiln` beside the Test262
corpus, never under `/tmp`.

When measuring a frozen compiler from another checkout, set `KILN_SOURCE_REV` to
its build source revision. Reports retain that declaration separately from the
harness checkout revision and record the actual compiler binary SHA-256. An
unknown compiler source revision stays unspecified.
