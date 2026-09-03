# Working in this repository

`CLAUDE.md` is a symlink to this file. Everything here is a rule that has already cost someone
hours; the dated narrative lives in `NOTES.md`, which is a log and not a manual.

## Building

### Use `mbx`, not bare `cargo`

`mbx build`, `mbx test`, `mbx clippy` — it takes any Cargo command and passes the subcommand and
its arguments through unchanged, so `mbx test -p kiln-cli --offline` is exactly `cargo test -p
kiln-cli --offline` with two things added: one content-addressed cache shared across every
checkout, and a single CPU budget shared by concurrent invocations.

That second property is the one that matters here, because this repository is routinely built by
more than one agent at once. Bare `cargo` does not know the others exist and will happily take
every core; several of them will thrash a machine that `mbx` would have kept busy. Reach for bare
`cargo` only when something genuinely needs it, and say why.

`cargo fmt` has no wrapper — call it directly.

### The toolchain comes from mise

`rust@1.97.1` and `mbx` are pinned in the global mise config. Do not install Rust tooling by hand,
and do not stub a tool to test a fallback: put a stub in a directory and prepend it to `PATH` for
one command. (NOTES.md records ninety minutes lost to a stubbed `rustup` that resolved back into
itself.)

## Measure in a worktree you own

`git worktree add ../kiln-yours -b your/work`. It costs a checkout, shares the mbx cache, and is
the single highest-value habit in this repo.

Several sessions write to the main checkout continuously, so a number taken there is a clock
reading rather than a measurement. Every wrong result recorded in `NOTES.md` on 2026-08-31 — test
counts that changed three times in ten minutes, "main is red" versus "main is green", a
regression that did not reproduce because the binary was three commits old — came from measuring a
tree something else was writing. A commit name is not a description of your working tree; `git
show HEAD:path` and `grep path` answer different questions.

Two more habits from the same day: check a binary's mtime against `git log -1` before contradicting
someone, and run `git log --oneline A..B` before blaming a commit, because alternating two
endpoints cannot tell you which of the commits between them did it.

## Running the test corpora

Each corpus declares itself either through a `manifest.toml` beside the files or through Test262
frontmatter, and several have a dedicated runner that knows flags `kiln test` will not guess.
**Use the runner in this table**; the wrong one produces confident, wrong numbers.

| Corpus | Command |
| --- | --- |
| `conformance/guard`, `conformance/effect` | `kiln test <dir>` |
| `conformance/regress` | `kiln test conformance/regress --node` |
| `conformance/node` | `kiln test conformance/node --node` |
| `conformance/server` | `kiln test conformance/server --node` |
| `conformance/native` | `scripts/native.sh` |
| `conformance/differential` | `scripts/differential.sh` |
| `conformance/thirdparty` | `scripts/thirdparty.sh` |
| Test262 | `scripts/test262.sh` |

`conformance/regress` is in the `--node` list for one test: `r08-node-async-context` builds a
`util.TextEncoder` and an `AsyncLocalStorage`, so it needs the adapter like any other Node test.
Without the flag it now says so -- it declares `requires_host = "node"` -- rather than failing with
`require is not defined`, which named neither the cause nor the fix.

`--node` installs the Tokio-owned Node adapter. Without it the Node and server corpora fail every
test rather than skipping, which reads exactly like a broken engine — it has been reported as one.
A test that needs the adapter now declares `requires_host = "node"` in its manifest, and without
the flag each such test fails saying so, so the reason distribution reads "35 × re-run with
`--node`" instead of thirty-five separate mysteries.

`conformance/thirdparty` holds esbuild bundles, not Test262 files. `kiln test` refuses it by name
and points at `scripts/thirdparty.sh`.

**Test262 runs need a memory cap.** `scripts/test262.sh` pins its own checkout and runs the suite
in a single `kiln test`, and that one process has repeatedly grown until it filled the disk. Run it
under an explicit cap and watch it; do not point `kiln test` at the corpus by hand.

## Changing Node HTTP close behavior

Run `bash scripts/check-http-close-contract.sh`. HTTP close authority spans the runtime transport,
the provider FFI, generated C and a real native socket; testing only the crate you edited can miss
the response-before-close ordering that `server.close()` depends on. The contract command runs the
compile-fail authority checks and all four executable seams in order, and is intentionally much
smaller than the workspace suite.

## The native number provider is a staticlib, and `cargo build` does not build it

`crates/kiln-native-number-ffi` declares `crate-type = ["rlib"]`, which is what workspace tests
consume. The `.a` that emitted C links against is a *different* build of the same crate —
`cargo rustc --crate-type staticlib --features regexp,bigint,temporal`, with its own RUSTFLAGS —
produced by `scripts/package-native-number-provider.sh`.

The consequence: a change to the provider ABI can leave the workspace build completely clean while
every native link fails with `Undefined symbols for architecture arm64`. `regexp`, `bigint` and
`temporal` are off by default, and dropping just `bigint` and `temporal` leaves an archive missing
24 of the 69 exports — measured, by building one that way.

The packager now verifies the archive's symbol table against `PROVIDER_EXPORTS` and names the
missing symbols, so this fails at packaging time with the fix in the message. If you are chasing
undefined symbols anyway, run `scripts/check-native-number-ffi.sh`.

## When the disk fills

The volume is shared with unrelated data and has hit 100% more than once; a link then fails with
`ld: write() failed, errno=28`, and below about 1 GiB free `fork` itself starts failing.

Reclaim **in this order**:

1. `target/debug/incremental` and `target/mbx-incremental` **in your own worktree** — pure
   regenerable scratch, the cheapest space there is.
2. `kiln cache gc` — bounds `.kiln/cache`, which has reached 6.7 GiB on an ordinary afternoon.
3. `mbx gc --max-size 10GiB` — `--dry-run` first; a missing cached object is simply rebuilt.

`mbx gc` is **last**, not first: run early it deletes *live* target directories belonging to other
worktrees. **Never delete build outputs you do not own.** An agent did that and destroyed two other
worktrees' builds.

## Agent skills

### Issue tracker

Issues and PRDs are repository-local Markdown under `.scratch/<feature>/`. See
`docs/agents/issue-tracker.md`. Scratch files there are runnable — `kiln run .scratch/probe.mjs`
works even though the project walk skips dot-directories.

### Triage labels

Use the standard five-role triage vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository with domain language in `CONTEXT.md` and decisions in
`docs/adr/`. See `docs/agents/domain.md`. Check `ls docs/adr/` before claiming a number — they are
shared space and have collided twice.
