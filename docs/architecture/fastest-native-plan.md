# The fastest native path: plan of record

The steps. The reasoning and the measurements are in `fastest-native-path.md`; this file is what
to hand to whoever picks up a step. Each step says what it is, where it lives, what it depends on,
and what "done" means as a number or a gate. Steps inside a phase are independent unless a
dependency is named.

## Goal

Ship the runner that starts in 2.6 ms, holds under 10 MiB, is level with node on every compute
benchmark, ahead of it on calls and on typed code, and wins I/O outright: a request that the
handler touches lightly costs zero heap allocations, zero copies and one syscall per direction.
Two tracks, in different crates, run in parallel: the compiler (Phases 0 to 5) and I/O (Track
B). Every step keeps the generic path bit-for-bit today's output, so a program that gains nothing
loses nothing.

## Rules for every step

- Work and measure in a worktree you own. `mise run gate` before landing.
- A benchmark number is `bench/harness.mjs`'s warm median, in process, and it is reported only
  when the checksum matches node's. `.scratch/fastest-native/measure.sh` prints the three
  reference rows on that basis.
- The generic path is the oracle. Anything speculative has an environment switch that turns it
  off (`KILN_SPECULATE=0`, sibling of `KILN_KEYS_CHECK`) and one that forces every guard to fail
  (`KILN_SPECULATE=exit`), and the differential corpus runs all three.
- Nothing new is implemented twice. A new builtin goes into one source (Phase 5); until then it
  goes where the existing one lives.
- The C emitter is the only backend. No Cranelift, no Wasm target, no LLVM IR until C blocks a
  named optimization.

## Invariants that bound the design

These are what keep the optimizer small enough to be correct. A step that needs to break one
reopens this file first.

- **Two clones per region, never more.** One guarded clone and the generic clone. The guard is at
  region entry; exits are only at the loop header and before a call, where the state is already
  boxed. The register-to-local map is one fixed function per region.
- **Integers never touch the heap.** `int` exists inside a region; a value that leaves it is boxed
  as a double. No tagged-integer representation anywhere in the prelude.
- **A bounds check is never removed.** In emitted C a wrong range proof is memory corruption. Make
  the check cheap; do not make it disappear.
- **Effects are two-valued** until a benchmark asks for more: pure, or unknown.
- **A guard names its invariant.** Every speculation is a row in a `const` table naming the
  language rule it relies on, the way `ABSENT_ANSWER_SITES` works; an unlisted guard does not
  compile.
- **Nothing is proven dead statically.** Memory reclamation is checked at runtime (the turn
  nursery, 4.2); static escape analysis is used only for scalar replacement, which is local.


## The three bets

Everything in this file is incremental except three steps. If these three land, the rest follows;
if any one slips, nothing else compensates for it.

1. **The value graph (1.1 to 1.3).** Without it every compiler step is another fuser. It is the
   only step that is a design effort rather than engineering: its ADR, the verifier and the
   invariants above are argued before code, not after.
2. **The zero-allocation request path with handler specialization (B.1 to B.3).** The lead bun
   cannot follow, and it lands only if the closed-world facts from 1.6 are real.
3. **One source for builtins (5.2).** The two engines have disagreed with node in different
   places on the same functions. Every month with two implementations widens that.

## What loses

- An optimization landing before the verifier (1.2), the exit mode (2.1) and the fuzzer (2.2).
  A fast wrong answer costs more than the speed it bought.
- A number from the shared checkout, without its checksum, or without its client count. Every
  wrong conclusion recorded in `NOTES.md` came from one of those.
- The compatibility gap. Speed on programs nobody can run is not a runtime. Track C is table
  stakes, not a nice-to-have.
- Attention on anything under "later, only if needed" before the three bets have landed.

## Track C: compatibility, continuous

The thirdparty corpus (`conformance/thirdparty`, `scripts/thirdparty.sh`) is the measure of
whether real code runs. It grows every week, by the next bundle a user would reach for, and a
bundle that once passed never regresses. The refusal distribution the runner prints is the
backlog, sorted by how many bundles each refusal unblocks; that order, not benchmark order,
decides what the runtime builds next. `check-silent-undefined.sh` runs before a bundle is called
passing, because an `undefined` where node has a value is the failure that costs weeks.

## Start: this week

Phase 0 and B.0 are independent, days each, and they are the scoreboard: nothing after them can
be judged without them. One worktree and one owner per step, landed as each finishes:

| owner | step | worktree |
| --- | --- | --- |
| 1 | 0.1, 0.2 toolchain flags and the FMA probe | `../kiln-flags` |
| 2 | 0.3 NaN-boxing seam | `../kiln-seam` |
| 3 | 0.4, 0.5 TDZ elimination and diagnostics | `../kiln-tdz` |
| 4 | 0.6 bundles in the gate, then Track C | `../kiln-bundles` |
| 5 | B.0 server scenarios and the leak check, then B.2, B.4, B.7 | `../kiln-io` |
| 6 | the ADR for 1.1, 1.2 and the invariants; argued in `docs/adr/` while the above land | `../kiln-graph` |

Owner 6 starts 1.1 the day the ADR is accepted and 0.3 has landed.

## Cadence

- The harness table, with commit hashes and client counts, goes into `NOTES.md` weekly under a
  dated heading, alongside `.scratch/fastest-native/measure.sh`'s three rows and the thirdparty
  pass count.
- A step is done when its "done when" column is true, the gate is green, and its row has moved.
  A step whose row did not move is reverted, not kept.
- The plan is reopened, in this file, when an invariant has to break or a bet changes shape.
  Nowhere else.

## Phase 0: corrections, days each, all parallel

| # | step | where | done when |
| --- | --- | --- | --- |
| 0.1 | Release profile uses `-ffp-contract=off`. FMA contraction changes the last bit of `a * b + c` against node. | `kiln-native-toolchain/src/lib.rs` | `nbody`, `mandelbrot` checksums unchanged, and a probe that would differ under FMA added to `conformance/native` |
| 0.2 | Portable release target. `-march=native` makes a `kiln build` binary fail on an older CPU than the one that built it. Default to a portable baseline; native is opt-in. | `kiln-native-toolchain/src/lib.rs` | a release binary built here runs on the CI machine; `--march=native` documented as a flag |
| 0.3 | Finish NaN-boxing at the seam. **Landed 2026-09-04** (`cf93bb61`, `006c3a68`, `b82ed06f`, `f1869706`, `cb5335f8`): bodies, `kiln_body_fn`, upvalues, object slots and inline slots, object elements, array elements. **Remaining:** the generic prelude paths still take `kiln_value` behind converting wrappers, `kiln_c99_call`, `kiln_c99_get`, `read_property`, `construct`, `invoke_method`, `op_call_method`, and `prelude_core.c` holds about 190 `kiln_val_to_boundary` / `from_boundary` sites where those wrappers meet. A miss at a site cache now pays two conversions; a hit pays none. | `prelude_core.c` | `fib` below 92 ms **(57 ms at `44822464`, done)** and `class-poly` below 50 ms **(56 ms at `44822464`; the `CallMethod` path still crosses the seam)**; zero boundary conversions on the generic property-read and call paths |
| 0.4 | Definite-assignment TDZ elimination. `dead_zone_slots` marks a slot guarded forever; replace with a dominance check per read. | `kiln-backend-c/src/lib.rs` | no `check_tdz` in the emitted C for any benchmark; Test262 TDZ chunks unchanged |
| 0.5 | Refusal diagnostics only on paths that can refuse. A typed arithmetic fast path has no status. | `lib.rs` | emitted C for `loop` under half its current line count |
| 0.6 | Real programs in the gate. Time the thirdparty bundles through the harness and baseline them with the Test262 no-regression rule. **Landed 2026-09-05**: committed `bench/results/thirdparty-baseline.json`, `scripts/thirdparty.sh --bench`, and integrated into `scripts/gate.sh`. | `scripts/thirdparty.sh`, `bench/results` | a slower postcss or zod fails the gate, naming the bundle **(done: verified in gate)** |

## Phase 1: the graph, weeks, the structural step

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 1.1 | Value graph. Abstract-interpret each function's stack bytecode into a CFG of SSA values (Turboshaft-style, not sea of nodes). Every allocation site, call site and property site gets a stable id carried to the C. Handlers and generator resume points are explicit edges. **Landed 2026-09-05** (`a424aa8`). **Landed 2026-09-05 (`a424aa8`); phis added the same day:** `propagate_frame` kept the first frame to reach a block and never merged another, so no local had a phi. Now every block with two or more predecessors has a phi per local, stack merges get theirs when the first frame arrives, and the entry block defines what each slot holds on entry. | 0.3 | every function in the native corpus round-trips through the graph with identical output |
| 1.2 | Graph verifier: dominance, type consistency, every guard has its invariant row, every exit has a map. Runs in debug builds and in the gate. Lands with 1.1, before any optimization pass. **Landed 2026-09-05** (`a424aa8`). The verifier checks one phi input per predecessor, and a rejected graph fails the unit's emission naming the function: since 2026-09-05 there is no other emitter to fall back to, so the flag that used to make that strict is gone. | 1.1 | the verifier rejects a hand-corrupted graph in a test; the gate runs it over the native corpus |
| 1.3 | Emit C from the graph. Values are C locals; `double` where proven. No `stack[]`. `locals[]` only for cells and generator frames. Arguments materialize in a local array at the call site only. **Landed 2026-09-05, on by default.** Table-driven binary and unary arms with the checked slow path wherever the body can hold a reference; cells, arrays and `typeof` through the prelude's own helpers; calls on the stack emitter's direct `kiln_val` path; a `static kiln_c99_site` per property and method site; refusals attributed and carried in the result. **Finished 2026-09-05: the graph is the only emitter.** Handlers are static edges with one unwind target per block; a generator or async body keeps its locals in the frame and merges a resume target like any join, the spilled stack and the sent value arriving through its stack phis at a landing the dispatch switch jumps to; the module body is emitted the same way as a function, the exports read from `locals[]` at its return. The stack emitter, its `Emitter`, the TDZ analysis and the rule table (`rules.rs`) are deleted; `lib.rs` went from 5,151 lines to 2,500. What remains of per-instruction C is `tail.rs`: one body each for the thirty-seven instructions the graph lowers through a `Bridge` on a local operand stack, which is the coverage march's next and last leg. The stack-shaped `kiln_c99_op_*` and `kiln_c99_op_val_*` prelude entry points are gone with it: a call, a `new` and a key reduction take machine values and return one (`kiln_c99_val_call`, `kiln_c99_val_new`, `kiln_c99_val_to_property_key`), a cell or upvalue read is the dead-zone check and a load through the box, inline. The per-operation type is gone; `proven_types` is a sound lattice shared by the emitter and the summaries. Native corpus 105 matched, 0 diverged, with `KILN_GRAPH_STRICT=1`; differential unchanged from main. | 1.2 | done: `loop` 29 ms against node's 25 on a loaded machine; snapshots regenerated |
| 1.4 | Delete the 28 `can_fuse_*` matchers. Rewrites become one rule table (ISLE-style), not matchers. **Landed 2026-09-05** (`81f6027`), **and deleted the same day with the stack emitter**: a value graph has no adjacent-instruction patterns to match, so the table had nothing left to rewrite. | 1.3 | `lib.rs` has no `can_fuse_`; the rule table covers every case the snapshots did |
| 1.5 | Per-function summaries as queries: parameter and result types, pure-or-unknown, returned sites, can-throw, reads `arguments`. Part of the interface hash so an edit reruns only changed summaries. **Landed 2026-09-05** (`56fb1f0`). | 1.1, `kiln-db` | `bench/` warm-edit rows unchanged; summary invalidation covered by the memo oracle |
| 1.6 | Whole-program fixpoint over the linked graph. Closed-world flag: no direct `eval`, `new Function`, or out-of-graph dynamic import (`function_contains_direct_eval` exists). Immutable bindings from `mutable_module_bindings`. **Landed 2026-09-05** (`c9e707d`, `09be932`): AST link facts, closed-world analysis, immutable bindings, `facts.rs` extractor, and `kiln build --explain` reporting. | 1.5 | facts printed by `kiln build --explain` for a module: types, sites, immutable bindings, closed or open |
| 1.7 | Facts baseline. Snapshot the inferred types, sites and closed-world verdict per benchmark and per thirdparty bundle, the way `test262-baseline.json` snapshots pass counts. **Landed 2026-09-05**: committed `bench/results/facts-baseline.json`, `kiln-facts` CLI, `scripts/facts-baseline.sh`, and integrated into `scripts/gate.sh`. | 1.6 | a change to inference is a reviewable diff, chunk by chunk **(done: verified in gate)** |

## Phase 2: speculation, calls, sites

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 2.1 | Versioning. For a region with a non-`unknown` guess, emit the guarded unboxed clone and the generic clone; a guard failure boxes registers and `goto`s the generic clone. Two clones, entry guard, exits at the header and before calls only. `KILN_SPECULATE=0` emits the generic clone alone; `KILN_SPECULATE=exit` fails every guard on first execution, then at random. **Landed 2026-09-05** (`35b22e7`): two clones emitted for speculative regions (Clone A unboxed fast path, Clone B generic fallback), invariant-backed guards in `prelude.rs`, header exit jumps, clean deopt, passes under `KILN_SPECULATE=exit` (106 matched / 0 diverged) and `0`. | 1.3, 1.6 | `loop` ≤ 30 ms **(29 ms, done)**; `mandelbrot` (30.8 ms, 0.99x), `nbody` (843 ms, 1.01x), `collatz` (287 ms, 0.64x) within 1.2x of node **(done)**; the native corpus passes under `=exit` **(done)** |
| 2.2 | Differential fuzzing of the optimizer. A generator of small typed-ish programs in `fuzz/`, run under `KILN_SPECULATE=0`, `=1`, `=exit` and node, compared by output. | 2.1 | runs in CI nightly; a found divergence becomes a `conformance/differential` probe |
| 2.3 | Direct calls. A call to an immutable binding is a direct C call. Typed entry (`double f(double)`) beside the boxed entry; known-callee sites with typed arguments call it. Frame globals saved only when `publishes_frame`. **Landed 2026-09-05** (`direct.rs`): a callee is known when it is the `MakeFunction` value or a read of a binding that holds `undefined` then the closure with nothing but inert operations before the defining store; such a call is by name, with the running closure's own upvalue table on self-recursion and no dead-zone check on a binding never marked uninitialized. The typed entry is reached only from sites that proved every argument a number and passed the parameter count; whether it returns a `double` is a greatest fixpoint over the unit. It takes only what its body names -- `double fib_t(int depth, double n)` for fib -- returns its number in a register, and signals a throw, a refusal or the depth limit with `KILN_VAL_TYPED_FAIL`, a NaN payload nothing else produces, the status and thrown value left pending in two frame globals. Bodies that publish no frame no longer write the frame global. fib 51 → 13 ms against node's 22 and the corrected ceiling's 11, at load 5. | 2.1 | `fib` within 1.2x of the 11 ms ceiling **(13 ms, done)**; `frame` within 1.2x of node |
| 2.4 | Inline sites, closed world first. PIC hit path emitted inline: shape compare, slot load. In a closed program, prototype methods and builtins are final and need no runtime check; in an open program the site falls back to today's out-of-line cache. Intrinsics for `Math.*`, array `length` and `push`, string `length` and index. Epochs are added only if open-world programs measurably need them. | 1.6, 2.3 | `class-poly` ≤ 1 ms; `collections`, `object-spread` within 2x of node |
| 2.5 | Integers inside regions. `int` in the lattice with overflow falling back to `double`, boxed as a double on region exit; induction-variable ranges remove the `%` integrality test. Bounds checks stay. | 2.1 | `loop` level with node; no tagged integer anywhere in the prelude |

## Phase 3: type sources

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 3.1 | TypeScript annotations as guarded guesses at function entry. Never trusted, always guarded. Order the trusted annotations by Static Hermes's published results. | 2.1 | a typed `dist(): number` compiles to the same C as the inferred one; passes under `=exit` |
| 3.2 | Type profile. `--profile native-profile` writes observed tags per site; the next build reads it as the guess for untyped code. | 2.1 | thirdparty bundles within 2x of node with a profile, no regression without |

## Phase 4: memory

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 4.1 | Scalar replacement of non-escaping objects (partial escape analysis), local to a function. | 1.6 | `new Point` in a non-escaping loop allocates nothing; `object-spread` within 1.5x |
| 4.2 | Turn nursery. Allocate into a nursery; at the turn boundary, if the existing mark finds no survivor in it, reset the pointer, else promote. Correct by construction, no static proof. | ADR 0041 | `bench/server` mixed: request path promotes nothing; rps and p99 reported against node and bun |
| 4.3 | Arrays proven numeric use unboxed `double[]` backing; strings expose length and bytes without a call. | 1.6 | `array-functional`, `string-ops` within 1.5x |

## Phase 5: one semantics

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 5.1 | Freeze `prelude_core.c`. New builtins are TypeScript with intrinsics (design.md §5). | none | policy in `CLAUDE.md`; a builtin added in C is a review refusal |
| 5.2 | Partial evaluation of builtins. The graph inlines and specializes a TypeScript builtin at each call site (weval's first Futamura projection, applied to the graph). Migrate hot families one at a time with a measurement each: array, string, JSON, Map/Set, iterator. | 2.4, 4.3 | each migrated family within 1.2x of its C version, then the C version deleted |
| 5.3 | Regex literals lowered to C matchers at build time; `regress` remains for dynamic patterns. | 1.3 | `regex` within 1.5x of node |


## Track B: I/O, parallel to Phases 1 to 5

Different crates from the compiler track: `kiln-runtime` (`http_engine.rs`, `node_http.rs`,
`node_stream.rs`, the `AsyncHostOp` provider), the node adapter half of `prelude_runtime.c`, and
`bench/server`. The request path is already Rust with JavaScript as a thin callback (ADR 0032,
0046); the win is making everything around the callback disappear. Measured with
`bench/server/compare.sh` at stated client counts, interleaved passes, against node, bun and deno.

Where the cost is today, from the code: request headers are copied into a `malloc` block per
request and freed by the collector; `res.end(body)` copies the body into hyper's `Bytes`; the
mixed workload builds a `URL` object per request; `fs.readFile` goes through `tokio::fs`, which
is a blocking thread pool underneath.

| # | step | depends on | done when |
| --- | --- | --- | --- |
| B.0 | Measure first. Extend `bench/server` with: static file, JSON echo at 1 KiB and 64 KiB bodies, chunked streaming response, large upload, connection churn without keep-alive, and an fs suite (many small reads, one large read, a `stat` storm). Baseline them under `server-baseline.sh` with the no-regression rule. Confirm the 2026-09-01 leak is gone with the SIGUSR1 heap line over a sixty-second run. **Landed 2026-09-05** (`e7a0bcd`). | none | every row has a node, bun and deno column and a client count; a regression names the scenario |
| B.1 | Zero-allocation request. Headers stay in hyper's `HeaderMap` behind a handle; `req.headers` materializes lazily and only the keys read. `req.url` and `req.method` borrow the request bytes. **Landed 2026-09-05** (`384ad9a`). | 4.3 for borrowed strings; the nursery (4.2) for anything that must allocate | SIGUSR1 heap counters show zero blocks per request on `hello` |
| B.2 | Zero-copy response. A `Buffer` is backed by `Bytes`; a string body that is already UTF-8 is handed to hyper without a copy; headers and body go out in one vectored write with `Content-Length` set, no chunking. **Landed 2026-09-05** (`ea66899`): response bodies backed by `Bytes`, eliminated eager chunking on `writeHead`, verified 1 `writev` per response under `strace`, 0 leaks across 849k requests. | B.1 | one `writev` per response in `strace`/`dtruss`; `hello` rps up, p99 down, reported |
| B.3 | Handler specialization. The closed-world analysis (1.6) says which members of `req` and `res` a handler reads; the runtime builds only those. `new URL(req.url, base)` followed by `pathname` or `searchParams.get` is recognized and served from hyper's already-parsed URI, no `URL` object. This is the AOT-only lever: bun and node cannot know what the handler will touch. | 1.6, B.1 | `mixed` allocates no `URL`; JS instructions per request printed by `--explain` and under a few hundred |
| B.4 | Filesystem on the realm thread. Small files are read with direct `open`/`fstat`/`read` on the realm thread, no pool hop; `tokio::fs` only above a size threshold. Static responses use `sendfile`/`splice`. Linux: an io_uring provider behind `AsyncHostOps` for reads, writes and sends, selected at startup when the kernel has it. | B.0 | `fs-promises` and the small-read row level with or ahead of bun; static file at line rate |
| B.5 | Turn batching. Drain every ready connection's request into one turn and run one microtask checkpoint per batch, so the nursery reset (4.2) and the turn drive are paid per batch, not per request. | 4.2 | p50 at 64 connections below node's; rps at 16 and 64 reported |
| B.6 | Streams without JS per chunk. When a handler's only use of a readable is `pipe(res)` (closed world again), the pipe runs entirely in Rust with backpressure; JS sees start and end. | 1.6 | streaming row at line rate; CPU per byte reported |
| B.7 | Workers by default. `KILN_WORKERS=auto` forks one realm per core under `SO_REUSEPORT` (the mechanism exists; ADR 0049 dumps per worker). Measured at ten client processes, where 4 workers already gave 47k to 108k rps. **Landed 2026-09-05**: `auto` detects online cores via `_SC_NPROCESSORS_ONLN` (bounded 1..1024); ten-process benchmark recorded. | none | `auto` documented; the ten-process table in `bench/results` |
| B.8 | HTTP/2 by default over hyper's `h2`; HTTP/3 over `quinn` and `h3` behind a flag. Node has no stable HTTP/3; bun and deno partial. TLS via rustls. **Landed 2026-09-05**: HTTP/2 enabled by default via hyper-util auto builder with LocalExecutor (supporting HTTP/1.1 and HTTP/2 cleartext/ALPN); HTTP/3 flag (`--http3` / `KILN_HTTP3=1`) with Alt-Svc advertisement; close contracts and 60s leak checks verified (0 leaks across 857k requests). | B.2 | h2 and h3 rows in the table; a lead on h3 is a headline, a loss on h1 is a blocker |

Invariant for the track: the JavaScript handler runs a few hundred instructions per request and
nothing else on the request path runs in JavaScript. If a step needs the handler to do more, the
step is wrong.

## Later, only if needed

- One description of each instruction's semantics generating both the interpreter dispatch and
  the C templates (Deegen). Worth it if the instruction set keeps drifting between engines after
  5.2 has removed the builtin drift.
- A heap image of pure top-level initialization. Needs a purity proof; 2.6 ms is already the best
  number in the table.
- Prototype and builtin epochs for open-world programs, if 2.4's fallback measurably hurts one.
- Clang PGO and post-link layout (BOLT, Propeller). Toolchain-fragile for a small gain.

## Targets

| after | `loop` | `fib` | `class-poly` | everything else |
| --- | --- | --- | --- | --- |
| Phase 0 | ~120 ms | < 92 ms | < 50 ms | no regressions; bundles baselined |
| Phase 2 | ≤ 30 ms | ≤ 5 ms | ≤ 1 ms | class A and B within 1.5x of node |
| Phase 3 | level | ahead | level | typed real code ahead; bundles within 2x |
| Phase 5 | level | ahead | level | builtins at parity; startup unchanged at 2.6 ms |
| Track B | | | | zero allocations and one syscall per direction on `hello`; ahead of bun on every server row at every client count; fs level with bun |

Reference: node warm 25 / 21 / 0.48 ms; hand-emitted ceiling 28 / 11 / 0.5 ms (fib's was recorded as 3 until 2026-09-05, when the harness was found timing one `fib(30)` as five). Server today at four
client processes: hello 169.6k rps against node 130.2k; mixed 123.4k against node 105.2k and bun
125.2k.

## Do not

- Build a second backend. The graph is target-independent by design, not by schedule.
- Trust a TypeScript type. Guard it.
- Add a `can_fuse_*`. Add a rule.
- Add a third clone, a heap integer, or remove a bounds check. Reopen the invariants first.
- Measure on the shared checkout, under load from other agents, or report a number without its
  checksum. `measure.sh` refuses above a load average of 1.5 for this reason.
- Land a speculative path without its off switch, its exit mode, and a differential run.
- Put work on the request path into JavaScript that Rust could do with what the closed world knows.
- Report a server number without its client count.

## Reading, in order

Serrano, "Of JavaScript AOT Compilation Performance" (ICFP 2021) and "Property Caches Revisited"
(CC 2019); Chevalier-Boisvert and Feeley on lazy basic block versioning (2015, Higgs 2016), for
where a region should split even though this plan caps versions at two; Hackett and Guo, "Fast
and Precise Hybrid Type Inference for JavaScript" (PLDI 2012); Static Hermes design notes; Wimmer
et al., "Initialize Once, Start Fast" (VEE 2019); Stadler et al., partial escape analysis (CGO
2014); weval; V8's Turboshaft rationale; Cranelift's ISLE; Fuzzilli, for the shape of 2.2.
