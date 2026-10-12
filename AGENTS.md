# Working in this repository

This is the manual for contributors and coding agents. Everything here is a rule that has already
cost someone hours; dated narrative belongs in `NOTES.md` and `docs/notes/`, which are logs and not
manuals.

## Building

### Use `mbx`, not bare `cargo`

`mbx build`, `mbx test`, `mbx clippy` — it takes any Cargo command and passes the subcommand and
its arguments through unchanged, so `mbx test -p kiln-cli --offline` is exactly `cargo test -p
kiln-cli --offline` with two things added: one content-addressed cache shared across every
checkout, and a single CPU budget shared by concurrent invocations.

That second property matters because this repository is routinely built by more than one agent at
once. Bare `cargo` does not know the others exist and will happily take
every core; several of them will thrash a machine that `mbx` would have kept busy. Reach for bare
`cargo` only when something genuinely needs it, and say why.

`cargo fmt` has no wrapper — call it directly.

### The toolchain is pinned

`rust-toolchain.toml` and `.mise.toml` pin Rust to `nightly-2026-09-15` (the type checker, a port
of Bun's, uses `allocator_api`). `mise install` fetches it; `mbx` is a separate tool that you
install yourself (`scripts/lib.sh` falls back to plain `cargo` when it is absent). Do not stub a
tool to test a fallback: put the stub in a directory and prepend it to `PATH` for one command. A
stubbed `rustup` that resolved back into itself once cost ninety minutes.

## Measure in a worktree you own

`git worktree add ../kiln-yours -b your/work`. It costs a checkout, shares the mbx cache, and is
the single highest-value habit in this repo.

Other sessions may write to the main checkout continuously, so a number taken there is a clock
reading rather than a measurement: test counts that change between runs, "main is red" versus
"main is green", and regressions that do not reproduce because the binary is several commits old
all come from measuring a tree something else is writing. A commit name is not a description of
your working tree; `git show HEAD:path` and `grep path` answer different questions.

Check a binary's mtime against `git log -1` before contradicting someone, and run
`git log --oneline A..B` before blaming a commit, because alternating two endpoints cannot tell you
which of the commits between them did it.

## One command before you land: `mise run gate`

`scripts/gate.sh` assembles the checks below into a single thing to run; hand-assembled versions
get subtly wrong. `mise tasks` lists it beside the rest.

| task | what it does |
| --- | --- |
| `mise run gate` | crate tests, native parity, thirdparty bundles, the three corpora, routine Test262 |
| `mise run gate-full` | the same gate plus every extended Test262 case and the `postcss` performance bundle |
| `mise run gate-quick` | the first three -- the loop worth running while iterating |
| `mise run test262` | routine Test262, with deferred coverage reported explicitly |
| `mise run test262-extended` | the expensive section alone; periodic or affected-feature checks |
| `mise run test262-full` | every Test262 case, checked against the full chunk baseline |
| `mise run test262-update` | regenerate the baseline; deliberate, and the diff is the record |
| `mise run test262-fetch` | fetch the pinned corpus into `~/.cache/kiln`, once per machine |
| `mise run native-stress` | the native corpus again with the collector stressed on every allocation |
| `mise run native-realms` | each native probe as 1, 2 and core-count realms at once in one process |
| `mise run native` | the native backend against node, probe by probe |
| `mise run thirdparty` | real library bundles, both engines |
| `mise run bench` | every microbenchmark under `bench/`, under kiln, node and bun |
| `mise run realworld` | real-world TypeScript programs under kiln, node, bun, porffor and Hermes |
| `mise run wpt` | the committed Web Platform Tests baseline through both engines |
| `mise run wpt-update` | regenerate the WPT baseline; deliberate, and the diff is the record |
| `mise run wpt-fetch` | fetch the pinned WPT directories into `~/.cache/kiln`, once per machine |
| `mise run http-close` | the Node HTTP close contract, across all four seams |
| `mise run gui-smoke` | kiln:gui against the real window system (macOS; needs a logged-in session) |
| `mise run kiln-types` | `@kiln/types` matches the `kiln:` modules' sources; its usage test and the GUI examples type-check |
| `mise run silent-undefined` | the expensive native-`undefined` class; several hundred compiles, not per-commit |
| `mise run tsc-differential` | the source checker against `tsc` 7.0.2 on Bun's differential corpus; fails if a baseline case stops matching |
| `mise run tsc-differential-update` | regenerate that baseline; deliberate, and the diff is the record |
| `mise run ts-conformance` | TypeScript's own compiler and conformance tests through the source checker, graded against typescript-go's `.errors.txt`; fails if a test drops below its recorded level |
| `mise run ts-conformance-update` | regenerate that baseline; deliberate, and the diff is the record |
| `mise run facts` | check the committed whole-program facts baseline for benchmarks and bundles |
| `mise run facts-update` | regenerate the facts baseline; deliberate, and the diff is the record |
| `mise run npm-compat` | npm package compatibility corpus; informational, not a gate |
| `mise run effect-upstream` | the pinned Effect test suite as direct TypeScript, Bun versus native (`scripts/effect-upstream.py`) |
| `mise run issues-index` | regenerate `docs/issues/README.md`, the open/closed index of the tracker |
| `mise run release-package` | build and smoke-test the release archive for this host into `dist/` |

`mise trust` once per checkout, or the tasks are invisible.

### Before every push: `scripts/pre-push.sh`, as a hook

There is no push-triggered CI: commits land on `main` faster than a run finishes, so the blocking
loop is local. `scripts/pre-push.sh` runs the HTTP close contract, the native provider contract and
the core crate tests, each only when the commits being pushed touch what it checks; `--all` runs
everything. `mise run hooks` points the checkout's `core.hooksPath` at `.githooks`, after which it
runs before every `git push`; `mise run pre-push` runs it by hand. `KILN_SKIP_PRE_PUSH=1` bypasses
the hook for a deliberate exception. A warm run of the crate tests takes a few minutes; the step
is bounded (40 minutes, `KILN_PRE_PUSH_TIMEOUT_MINUTES`) and native test fixtures are killed after
60 seconds, so a hung test fails the push naming itself rather than holding it. The only GitHub workflow is `.github/workflows/release.yml`,
for manual and tag releases.

`conformance/test262-baseline.json` holds a pass/fail count per chunk. A whole-corpus number is one
figure and hides everything -- a change that wins forty rows in one chunk while losing forty in
another reads as no change at all -- so the gate compares chunk by chunk and a regression names the
chunk it happened in. Moving those numbers is a diff a reviewer has to accept, next to the change
that caused it, exactly as `update-baseline.sh` intends for the performance baseline.

Two things the gate does that a hand-rolled one keeps getting wrong. It asserts on the *summary
line* of the crate tests rather than on an exit code, because a broken test build leaves a working
release binary and a suite that never ran. It prints one total for the workspace (`crate tests: N suites,
M passed, F failed`) and refuses below `KILN_GATE_MIN_PASSED` (the gate's printed total is authoritative),
because the last three summary lines it used to print are empty doc-test targets and read as a
suite that never ran. And it re-execs itself under a bash 4 or newer: macOS
ships 3.2, `mise run` finds it, and under 3.2 `thirdparty.sh` loses `BASHPID` and its cleanup trap
deletes the shared results directory instead of its own -- every bundle then reports "missing".

`mise run native-stress` is the native corpus with `KILN_GC_STRESS` set for the native side of
every pair. It is not in the gate and should not be: it is minutes rather than seconds, and it
answers a different question. The ordinary corpus asks whether the two engines agree; this asks
whether every value the native walk builds is reachable at every point a collection could happen,
because a value that is merely *usually* reachable passes the ordinary run every time. A
`JSON.parse` that returned one key's bytes under another key's name survived the ordinary corpus
(`conformance/native/json-shaped-build-edges.js`) for exactly that reason. Run it after anything that touches
allocation, rooting or the collector, and before believing a corpus that "already covers" the
shape you changed.

`mise run native-realms` (`scripts/native-realms.sh`) runs every native probe's binary alone and
then with `KILN_REALMS=N`: N realms of the program at once in one process, each pinned to its own
thread, each required to print exactly what the program prints alone. It asks what the other two
cannot -- whether realms side by side see each other through the process-wide keys, shapes and
inline caches (ADR 0071). Run it after anything that touches shared cache state or its publication
(`KILN_C99_SLOT_*` in the prelude); name a representative subset of probes on the command line and
set `KILN_NATIVE_GC_STRESS=1` for the stressed variant, which is minutes for a subset and far longer
for the corpus.

### Keep the gate fast; run it once when the change is finished

The gate builds the CLI once, runs the artifact-cache suite before concurrent filesystem work,
then overlaps the remaining crate tests, native/thirdparty parity and Test262
within one budget (at most 32 CPUs, or `KILN_GATE_JOBS` / `KILN_JOBS` if smaller). Test262 keeps
one worker per GiB of its hard memory cap; the gate sizes that cap to its assigned workers with
a 4 GiB floor for single-case allocations, at most half physical memory or 16 GiB. An explicit
`KILN_TEST262_MEMORY_MB` is respected.
`KILN_GATE_SERIAL=1` runs the same checks serially for diagnosis. In a parallel non-quick gate, native
thirdparty benchmark binaries compile with one reserved worker alongside correctness lanes, then
wait at a gate-owned release marker; warmup and timed execution begin only after those lanes finish.
The routine thirdparty performance baseline measures every present baseline bundle except
`postcss`; `gate-full` adds `postcss`, whose native build is much slower. The benchmark log records
selected and deferred bundle counts. Thirdparty correctness
parity still covers every present probe in both gates. Every step records its elapsed seconds in
the gate evidence;
Test262 retains per-chunk result documents including individual case timings.
Test262 discovers an explicit file/variant inventory, then distributes bounded batches across
processes within the same CPU and memory allowance. Every batch retains original corpus paths,
harness includes, module fixtures and fresh realms. The runner verifies exact case IDs per batch
and across the selected suite before accepting its counts; crashes, missing or duplicate cases
fail the run. Complete per-chunk result documents remain available beside batch logs.

`conformance/test262-extended.json` is the reviewed list of expensive files, files whose
measured cost dominated case execution time. The everyday gate runs the complement. This is a cost classification, not a claim that these
tests must be slow: Unicode sweeps dominate it, and several entries expose optimization or
correctness problems. All variants of a file stay together. Selection uses the committed list;
timings only order batches and never silently omit a case.

The full chunk baseline remains unchanged. The extended manifest records its section's reference
counts; the routine passing floor is the full chunk floor minus that section's reference passes.
A full run checks the original full floor. Reports name the suite, selected/deferred/discovered
counts and manifest digest. `test262-update` requires full coverage and deliberately updates both
sets of counts. Changing the extended list requires a complete run and reviewed baseline split.

Backend C unit fixtures honor `CC` and cache compiled objects under
`target/kiln-backend-test-cache`. Every invocation still preprocesses the source (so included
headers affect the key), relinks against the current host libraries, and executes the fixture.
The key includes compiler identity, flags and preprocessed content. Test-only modules do not
participate in the production C emitter fingerprint; editing a fixture does not invalidate all
native artifacts. These are compilation caches, not cached test verdicts.
Four CLI HTTP execution/build fixtures likewise reuse artifacts under
`target/kiln-cli-native-semantic-cache`, while keeping fresh project directories and executing the
programs on every run. Explicit cache miss/hit tests retain private caches. Every worker pool
(source reads and parses, module parse and IR, CommonJS payloads, the source checker, native
emission) is a named rayon pool sized by `kiln-jobs`: the CPUs, its own cap, and any positive
`KILN_JOBS` or `RAYON_NUM_THREADS`; those are per-pool bounds, not a global limit on concurrent
processes. A new pool uses `kiln_jobs::budget` and `kiln_jobs::pool` rather than its own copy.
The test profile optimizes IR, graph and C emission passes at level 1 while retaining debug
assertions and overflow checks; ordinary dev builds keep their existing settings.
The cache suite keeps its full stress repetition and contributes to the same workspace totals;
it runs before the concurrent lanes because overlapping its durable writes with native compilers
made a subsecond suite take 45 seconds.

Run the routine gate once when a change is finished. Use `gate-full` for changes to this runner,
its classification or baseline, and periodically or before releases; use `test262-extended` or the
pinned subtree runner when changing the features those cases exercise. While iterating, use the
narrowest check that answers the question: one crate, one Test262 subtree, or affected native
probes. `gate-quick` has the workspace/native/thirdparty lanes. No mode caches test verdicts, and
routine coverage must never be reported as a complete conformance run. The target is a few
minutes; investigate slow steps instead of accepting a longer loop.

Keep the whole log when the gate runs in the background. A `tail` on `native.sh` can cut the
summary line above the refusal backlog, and on `thirdparty.sh` the name of the bundle that
diverged. Redirect each script to its own file and grep that.

Each gate invocation keeps its evidence under `target/gate-logs/run.<id>/` (or the directory named
by `GATE_LOG_DIR`). The directory contains `aggregate.json`, per-step result envelopes, and the
raw `.log`/`.raw` streams; conformance and Test262 steps also retain their result documents. Read
the aggregate for the overall verdict and the step envelope for parsed counts, then use the raw
log when the adapter reports an error or a test failure needs its full output.

Two habits that waste the most. Rebuilding and re-measuring on every commit that lands on `main`
-- a rebase costs a release build plus a cache-cold native compile of the prelude. And running the whole Test262
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

`--node` installs the Tokio-owned Node adapter. `kiln run` uses the wall clock by default (`Date`, `performance.now` and timers show real time, as
in node); `--clock virtual` selects the deterministic clock of ADR 0044. Anything that compares the
interpreter with the native backend, or with node, must pass `--clock virtual` explicitly -- the
scripts in this table already do. `kiln build` writes `./<name>` (not `.kiln/bin`) at
`native-release`; `kiln run --native` still builds under `.kiln/bin` at `native-dev`. The Node layer
turns on by itself when the program imports a `node:` built-in; `--node` forces it.

`conformance/regress` is in the `--node` list for
one test (`r08-node-async-context` needs `util.TextEncoder` and `AsyncLocalStorage`). A test that
needs the adapter declares `requires_host = "node"` in its manifest, and without the flag it fails
saying so rather than skipping, so a missing flag does not read as a broken engine.

`conformance/thirdparty` holds esbuild bundles, not Test262 files. `kiln test` refuses it by name
and points at `scripts/thirdparty.sh`.

**Test262 runs need a memory cap.** `scripts/test262.sh` pins its own checkout and runs the suite
or an explicit `--path` subtree in a single `kiln test`, and that one process has repeatedly grown
until it filled the disk. The script applies the cap to either form; do not point `kiln test` at the
corpus by hand.

The Test262 runners also bound interpreter worker concurrency within that process. They use at
most one Rayon worker per GiB of the configured memory cap (at least one), further bounded by
available CPUs and positive `KILN_JOBS` / `RAYON_NUM_THREADS` settings. At the default 4 GiB cap,
that means at most four workers. This is a scheduling allowance, not a per-test memory guarantee;
the hard process cap remains in force. Letting Rayon use every CPU can exhaust the capped address
space through overlapping test allocations and worker stacks even when each test fits alone.

**Keep the corpus outside `/tmp`.** Temporary directories are emptied by periodic cleaners (macOS
does it on a day boundary), which can take a whole checkout mid-session. `mise run test262-fetch`
fetches the pinned corpus into `~/.cache/kiln`, and `scripts/test262.sh` accepts
`KILN_TEST262_CHECKOUT` for a checkout elsewhere while still verifying the pinned commit. It is not
a submodule on purpose: tens of thousands of files multiplied by every worktree is the cost the
worktree habit above exists to avoid.

**A vanished corpus reads as a clean run.** `kiln test` on an emptied tree reports
`discovered 0 / pass 0 / fail 0` and exits 0 -- deliberately, because an empty corpus is not a
claim about the compiler -- so `0/0` chunks look like a catastrophic regression rather than a
missing corpus. Any gate script of your own must
refuse to run when the corpus is missing or short, instead of reporting zeros.

## Benchmarking: time the work, not the launcher

`mise run bench` runs every microbenchmark under `bench/` against kiln, node and bun.
`bench/server/compare.sh` is the separate one for HTTP throughput.

Naive timings flatter this repository, for two structural reasons:

* **Process startup sat inside every row.** node takes about 45 ms to boot where a kiln native
  binary takes 2.6 ms, so a benchmark whose work lasts 1 ms was mostly a comparison of launchers.
* **Nothing was warm.** A JIT has barely started after one call, so timing a single run compares
  warm ahead-of-time code against cold interpreted code and reports the difference as a property
  of the compiler. `array-functional` is several times faster on node and bun in
  steady state than on its first call, enough to flip the comparison.

So a benchmark exports its workload to `bench/harness.mjs`, which runs it many times in one
process, calibrates a batch size against the clock's resolution, and reports the first call apart
from the steady state. Setup belongs at module scope, outside the timed function -- with the
exception of state the workload destroys, like the array `array-sort` sorts in place, which has
to be rebuilt inside it or the second iteration measures an already-sorted best case.

**The interpreter cannot time itself.** (`kiln run` shows wall time by default now, so every
harness that compares engines or times the interpreter passes `--clock virtual`; `kiln test` is always
virtual.) Under that policy its realm clock is virtual and advances only for
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
takes the other branch, and the program dies three frames later naming nothing.

So the prelude C may not hand back `undefined` from a read by writing it. Use `KILN_C99_ABSENT`,
whose reason names the membership predicate that proves the name is really absent in the language,
and add the row to `ABSENT_ANSWER_SITES` in `crates/native/kiln-backend-c/src/prelude.rs`. Both are `const`
assertions: writing it by hand does not compile, and neither does a new site without a row.

That gate cannot check whether a predicate is *complete*, because completeness is a question about
the other engine. `bash scripts/check-silent-undefined.sh` asks it by running both, and exits
non-zero on this class alone. It costs several hundred native compiles, so it is not per-commit;
`scripts/native.sh` is, and `conformance/native/*-refuses.ts` is where a decision to keep refusing
gets written down so that the next person does not "fix" it into an `undefined`.

A refusal is the cheap answer *for today*, not the destination. kiln is a runtime, and ADR 0063
makes every refusal a debt: node's surface is the target of both engines (the interpreter leads,
it is not the destination), the inventory's refused count per receiver is a baseline that only
goes down, and an artifact-size argument chooses how a
member gets linked (demand-driven, ADR 0062), never whether it exists. A refusal that stands long
enough tends to be "fixed" into the `undefined` above, so "refuses
by name" is a state to leave, in the order the thirdparty corpus and the inventory give.

## Editing the C prelude and native caches

Native generated-C, object, runtime and link keys include the content digests of the prelude
source or header they consume. The generated-module key uses a backend module-emitter fingerprint
that excludes separately compiled runtime C: runtime-C-only edits do not invalidate graph
generated-C records, while exact runtime source and prelude-header digests still invalidate
dependent objects and links. Editing `crates/native/kiln-backend-c/src/runtime/` therefore creates
cache misses for the affected layers; an ordinary build does not require manual cache deletion.
Concurrent native object misses coordinate through bounded process mutexes and advisory files in
`<first-writable-cache-layer>/.compile-locks/v1/` (at most 256 files). After waiting, each caller
rechecks the full artifact key; only a still-missing object compiles. Warm hits do no lock I/O.
These files are deliberately outside the artifact records swept by GC; retain them while builds
are active, because replacing a lock file would let two producers lock different inodes. The native
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

`crates/native/kiln-native-provider-ffi` declares `crate-type = ["rlib"]`, which is what workspace tests
consume. The `.a` that emitted C links against is a *different* build of the same crate —
`cargo rustc --crate-type staticlib --features regexp,bigint,temporal`, with its own RUSTFLAGS —
produced by `scripts/package-native-number-provider.sh`.

The consequence: a change to the provider ABI can leave the workspace build completely clean while
every native link fails with `Undefined symbols` errors. `regexp`, `bigint` and `temporal` are off by
default, and dropping any of them leaves an archive missing exports.

The packager now verifies the archive's symbol table against `PROVIDER_EXPORTS` and names the
missing symbols, so this fails at packaging time with the fix in the message. If you are chasing
undefined symbols anyway, run `scripts/check-native-number-ffi.sh`.

## The host GUI bridge is linked in, not found next to the program

A compiled GUI program does not `dlopen` a shared library found at run time. The bridge (`kiln-host-gui`, the `kiln_gui_*` operations)
is now part of the provider archive of every variant that declares the `gui` capability -- `gui`
and `full-gui` -- and the native `node:ffi` engine answers `dlopen("kiln:host-gui")` from a table
the archive carries (`kiln_host_gui::ffi::exports`). The specifier is not a path; nothing is
searched for.

Three consequences worth knowing before you touch it:

* **`full` does not include `gui`.** A headless program must never link AppKit, so `full` stops
  short of the bridge and `full-gui` is the variant above it. `kiln build` picks `full-gui` only
  when the live graph imports `kiln:gui`, including through the conservative CommonJS answer.
  `provider has ... gui` in the build report is how you tell.
* **The frameworks come from the contract, and the packager proves them.** `gui::link_arguments`
  in `kiln-native-provider-contract` lists what a `gui` archive adds to the link line
  (`-framework Cocoa ...` on macOS, nothing on Linux). rustc's `--print native-static-libs` is
  the authoritative source but is printed only by a compiler that actually ran, which a cached or
  fresh rebuild is not; so the packager diffs the note against the list when it appears and, always,
  links a probe that takes the address of every bridge export against the archive. A new
  framework dependency fails packaging with the symbol named, not some program's link.
* **The table is what keeps the bridge alive.** An upstream `#[no_mangle]` that nothing references
  is dropped from a staticlib by whole-archive LTO -- measured: zero `kiln_gui_*` symbols and no
  frameworks in the note. `exports()` takes every function's address; the contract's `GUI_EXPORTS`
  and the crate's own test hold the table to the source file, so a new operation that is not in
  both does not land.

`KILN_GUI_LIB=/path/to/libkiln_host_gui.dylib` still points a program at a shared library
instead, for iterating on the Rust side without the provider's fat-LTO rebuild (about two minutes
per edit to a listed provider source). It is a developer override, not a search path.

## When the disk fills

Builds here are large (tens of GiB of `target/` and `.kiln/` per worktree). A full disk shows up as
`ld: write() failed, errno=28`, and below about 1 GiB free `fork` itself starts failing.

Reclaim **in this order**:

1. `target/debug/incremental` and `target/mbx-incremental` **in your own worktree** — pure
   regenerable scratch, the cheapest space there is.
2. `kiln cache gc` — bounds `.kiln/cache`, which grows to many GiB in ordinary use.
3. `mbx gc --max-size 10GiB` — `--dry-run` first; a missing cached object is simply rebuilt.

`mbx gc` is **last**, not first: run early it deletes *live* target directories belonging to other
worktrees. **Never delete build outputs you do not own.**

mbx also collects on its own: with `gc.auto` on, once free space falls below `gc.min_free_size`
(default 10% of the disk) every build evicts other workspaces' managed targets, which reads as
gates failing on vanished binaries. Lower it with `mbx settings set gc.min_free_size 8GiB` on a
tight disk; `mbx doctor` shows the budget. If a build directory disappears under you, check `df`
and `mbx stats` before suspecting a colleague.

## Profiling a native binary

`scripts/profile-native.sh <binary> [top-n]` (macOS) records an Instruments CPU profile of a built
program and prints the hottest symbols and the hottest instruction offsets, for `objdump -d` to
read. Use it once `sample` names a single emitted function and the question is what inside it
is slow. Valgrind does not run on Apple Silicon; Instruments is the platform's profiler.

## Freeze `prelude_core.c`

`prelude_core.c` is frozen for new built-in implementations.
New built-ins must be written in TypeScript with intrinsics (`docs/design.md` §5, ADR 0005).
Adding a built-in method or function to `prelude_core.c` is a review refusal.
Existing C built-ins are being migrated to TypeScript.

`prelude_core.c` and `prelude_runtime.c` were later split, as a pure move, into the domain files
under `crates/native/kiln-backend-c/src/runtime/` (see `runtime/README.md` for the list and the order,
which is load-bearing). The freeze applies to all of them: a built-in added to any file in that
directory is the same review refusal.

## Agent skills

### Issue tracker

Issues and PRDs are repository-local Markdown under `docs/issues/<feature>/`. See
`docs/agents/issue-tracker.md`. Probes there are runnable -- `kiln run docs/issues/<feature>/probe.mjs`
works like any other path. Deleting a filing is refused by the hooks and the gate unless
`KILN_ALLOW_ISSUE_DELETION=1` is set for that one command; a closed issue keeps its file. Only
Markdown and small probe sources are committed there; logs, dumps and profiles stay local.
`.scratch/` is untracked scratch paper.

### Triage labels

Use the standard five-role triage vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository with domain language in `CONTEXT.md` and decisions in
`docs/adr/`. See `docs/agents/domain.md`. Check `ls docs/adr/` before claiming a number — they are
shared space and have collided twice.
