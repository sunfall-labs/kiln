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

## One command before you land: `mise run gate`

The pieces below all existed as separate scripts; what did not exist was a single thing to run, so
each agent assembled their own in a scratch directory and got it subtly wrong. `scripts/gate.sh` is
that assembly, in the repository, and `mise tasks` lists it beside the rest.

| task | what it does |
| --- | --- |
| `mise run gate` | crate tests, native parity, thirdparty bundles, the three corpora, the Test262 baseline |
| `mise run gate-quick` | the first three -- the loop worth running while iterating |
| `mise run test262` | the Test262 baseline alone, chunk by chunk |
| `mise run test262-update` | regenerate the baseline; deliberate, and the diff is the record |
| `mise run test262-fetch` | fetch the pinned corpus into `~/.cache/kiln`, once per machine |
| `mise run native-stress` | the native corpus again with the collector stressed on every allocation |

`mise trust` once per checkout, or the tasks are invisible.

`conformance/test262-baseline.json` holds a pass/fail count per chunk. A whole-corpus number is one
figure and hides everything -- a change that wins forty rows in one chunk while losing forty in
another reads as no change at all -- so the gate compares chunk by chunk and a regression names the
chunk it happened in. Moving those numbers is a diff a reviewer has to accept, next to the change
that caused it, exactly as `update-baseline.sh` intends for the performance baseline.

Two things the gate does that a hand-rolled one keeps getting wrong. It asserts on the *summary
line* of the crate tests rather than on an exit code, because a broken test build leaves a working
release binary and a suite that never ran. It prints one total for the workspace (`crate tests: N suites,
M passed, F failed`) and refuses below `KILN_GATE_MIN_PASSED` (500; the workspace runs about 1,440),
because the last three summary lines it used to print are empty doc-test targets and read as a
suite that never ran. And it re-execs itself under a bash 4 or newer: macOS
ships 3.2, `mise run` finds it, and under 3.2 `thirdparty.sh` loses `BASHPID` and its cleanup trap
deletes the shared results directory instead of its own -- every bundle then reports "missing" and
the change under test takes the blame.

`mise run native-stress` is the native corpus with `KILN_GC_STRESS` set for the native side of
every pair. It is not in the gate and should not be: it is minutes rather than seconds, and it
answers a different question. The ordinary corpus asks whether the two engines agree; this asks
whether every value the native walk builds is reachable at every point a collection could happen,
because a value that is merely *usually* reachable passes the ordinary run every time. A
`JSON.parse` that returned one key's bytes under another key's name survived
`conformance/native/json-shaped-build-edges.js` -- which nests two hundred deep -- for exactly
that reason: the collection never landed inside the window. Run it after anything that touches
allocation, rooting or the collector, and before believing a corpus that "already covers" the
shape you changed.

### Run the gate once, at the end, not as a progress bar

The full gate is expensive: several hundred native compiles, the thirdparty bundles, three corpora
and the Test262 baseline, and several agents share one machine and one CPU budget. Run it **once,
when the change is finished**, and not after every edit. While iterating, run the narrowest thing
that can answer the question you actually have: the one crate's tests, `scripts/native.sh` for a
prelude change, `kiln test` on the one corpus directory the change touches, or the single probe
that failed. `gate-quick` is the widest loop worth running mid-change, and even that is minutes.

Keep the whole log when the gate runs in the background. A `tail -8` on `native.sh` kept the
refusal backlog and cut the one summary line above it, and the same on `thirdparty.sh` kept the
verdict and cut the name of the bundle that diverged; both corpora had to run again to read what
they had already printed. Redirect each script to its own file and grep that.

Two habits that waste the most. Rebuilding and re-measuring on every commit that lands on `main`
-- a rebase costs a release build plus a cache-cold native compile of the prelude, and the number
you get is a clock reading unless the change under test is yours. And running the whole Test262
baseline to check a change that touches one chunk; `scripts/test262.sh --out <report> --path
<subtree>` runs a pinned subtree with the same memory cap as the whole-corpus command.

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
or an explicit `--path` subtree in a single `kiln test`, and that one process has repeatedly grown
until it filled the disk. The script applies the cap to either form; do not point `kiln test` at the
corpus by hand.

**Keep the corpus outside `/tmp`.** A scratchpad checkout under `/private/tmp` is on borrowed time:
macOS's periodic cleaner empties it on a day boundary, and on 2026-09-04 it took the whole
checkout, `.git` included, in the middle of a session. Keep one at
`~/.cache/kiln/test262-<short-commit>` and point work at it -- `scripts/test262.sh` already accepts
`KILN_TEST262_CHECKOUT` and still verifies the pinned commit, so a durable checkout costs no
guarantee. A submodule was considered and rejected: 53,874 files multiplied by every worktree is
exactly the cost the worktree habit above exists to avoid, and the pin is already enforced.

**A vanished corpus reads as a clean run.** `kiln test` on an emptied tree reports
`discovered 0 / pass 0 / fail 0` and exits 0 -- deliberately, because an empty corpus is not a
claim about the compiler -- so seven chunks answering `0/0` look like a catastrophic regression in
whatever you were measuring rather than like a missing corpus. Any gate script of your own must
refuse to run when the corpus is missing or short, instead of reporting zeros.

## Benchmarking: time the work, not the launcher

`mise run bench` runs every microbenchmark under `bench/` against kiln, node and bun.
`bench/server/compare.sh` is the separate one for HTTP throughput.

The numbers it replaced were wrong in a way that flattered this repository, and both causes are
structural rather than bad luck on the day:

* **Process startup sat inside every row.** node takes about 45 ms to boot where a kiln native
  binary takes 2.6 ms, so a benchmark whose work lasts 1 ms was mostly a comparison of launchers.
* **Nothing was warm.** A JIT has barely started after one call, so timing a single run compares
  warm ahead-of-time code against cold interpreted code and reports the difference as a property
  of the compiler. `array-functional` is 4.8x faster on node and 6.9x faster on bun between its
  first call and its steady state -- measured once, the native backend looked 1.3x slower than
  node on it; measured warm, it is 11x.

So a benchmark exports its workload to `bench/harness.mjs`, which runs it many times in one
process, calibrates a batch size against the clock's resolution, and reports the first call apart
from the steady state. Setup belongs at module scope, outside the timed function -- with the
exception of state the workload destroys, like the array `array-sort` sorts in place, which has
to be rebuilt inside it or the second iteration measures an already-sorted best case.

**The interpreter cannot time itself.** Its realm clock is virtual and advances only for
scheduled work (ADR 0044), so `Date.now`, `performance.now` and `process.hrtime` all return the
same number from the first line of a synchronous run to the last. That is deliberate -- it is
what makes a run reproducible -- so the harness detects it, reports iterations instead of
milliseconds, and `compare.sh` times those from outside against a measured floor. Do not write a
`while (Date.now() - start < n)` spin anywhere near the interpreter: against a clock that never
moves it never ends.

**A checksum that differs from node's is a divergence, not a time.** Every runtime's answer is
compared, and one that did different work is excluded from the table and named. A fast wrong
answer reads exactly like a win.

## Changing Node HTTP close behavior

Run `bash scripts/check-http-close-contract.sh`. HTTP close authority spans the runtime transport,
the provider FFI, generated C and a real native socket; testing only the crate you edited can miss
the response-before-close ordering that `server.close()` depends on. The contract command runs the
compile-fail authority checks and all four executable seams in order, and is intentionally much
smaller than the workspace suite.

## An `undefined` from the native backend is the expensive answer, not the cheap one

The compiled backend refuses what it has not built, and a refusal names the member and stops the
program at the cause. The failure that costs money is the other one: a member read that answers
`undefined` where the interpreter has a value. Nothing stops, `typeof x.member === "function"`
takes the other branch, and the program dies three frames later naming nothing. A bundled `postcss`
died on `path.dirname` that way for weeks.

So the prelude C may not hand back `undefined` from a read by writing it. Use `KILN_C99_ABSENT`,
whose reason names the membership predicate that proves the name is really absent in the language,
and add the row to `ABSENT_ANSWER_SITES` in `crates/kiln-backend-c/src/lib.rs`. Both are `const`
assertions: writing it by hand does not compile, and neither does a new site without a row.

That gate cannot check whether a predicate is *complete*, because completeness is a question about
the other engine. `bash scripts/check-silent-undefined.sh` asks it by running both, and exits
non-zero on this class alone. It costs several hundred native compiles, so it is not per-commit;
`scripts/native.sh` is, and `conformance/native/*-refuses.ts` is where a decision to keep refusing
gets written down so that the next person does not "fix" it into an `undefined`.

## Editing the C prelude and native caches

Native generated-C, object, runtime and link keys include the content digests of the prelude
source or header they consume. The backend emitter build fingerprint also participates in the
generated-module key. Editing `crates/kiln-backend-c/src/runtime/` therefore creates cache misses
for the affected layers; an ordinary build does not require manual cache deletion. The native
tests use a separate cache root under `target/kiln-test-cache`, but it follows the same content
addressing rules.

If a record is found under the same key with a different payload, the cache reports
`corrupt cache artifact <path>: same cache key already contains a different payload` and stops.
That is an integrity failure, not a stale-prelude recovery path: preserve the full diagnostic,
inspect the key and payload provenance, and remove only the exact bad cache layer or record after
confirming the cause. `kiln cache gc` bounds `.kiln/cache`; it does not change cache identity.

Run `scripts/native.sh` and `scripts/thirdparty.sh` by **executing** them, never as `bash
scripts/native.sh`: macOS's `/bin/bash` is 3.2, the cleanup trap loses `BASHPID` under it and
deletes the shared results directory instead of its own, and every probe then reports a missing
result file. `mise run gate` gets this right; a hand-run does not unless you copy it.

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

## Profiling a native binary

`scripts/profile-native.sh <binary> [top-n]` records an Instruments CPU profile of a built
program and prints the hottest symbols and the hottest instruction offsets, for `objdump -d` to
read. Use it once `sample` names a single emitted function and the question is what inside it
is slow. Valgrind does not run on Apple Silicon; this is the platform's profiler.

## Freeze `prelude_core.c` (Phase 5.1)

`prelude_core.c` is frozen for new built-in implementations.
New built-ins must be written in TypeScript with intrinsics (design.md §5, ADR 0005).
Adding a built-in method or function to `prelude_core.c` is a review refusal.
Existing C built-ins are migrated to TypeScript in Phase 5.2.

`prelude_core.c` and `prelude_runtime.c` were later split, as a pure move, into the domain files
under `crates/kiln-backend-c/src/runtime/` (see `runtime/README.md` for the list and the order,
which is load-bearing). The freeze applies to all of them: a built-in added to any file in that
directory is the same review refusal.

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
