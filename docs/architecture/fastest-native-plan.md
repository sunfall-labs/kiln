# The fastest native path: plan of record

The steps. The reasoning and the measurements are in `fastest-native-path.md`; this file is what
to hand to whoever picks up a step. Each step says what it is, where it lives, what it depends on,
and what "done" means as a number or a gate. Steps inside a phase are independent unless a
dependency is named.

## Goal

Ship the runner that starts in 2.6 ms, holds under 10 MiB, is level with node on every compute
benchmark, ahead of it on calls and on typed code, and leads on server throughput by a margin node
cannot close. Every step keeps the generic path bit-for-bit today's output, so a program that
gains nothing loses nothing.

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

## Phase 0: corrections, days each, all parallel

| # | step | where | done when |
| --- | --- | --- | --- |
| 0.1 | Release profile uses `-ffp-contract=off`. FMA contraction changes the last bit of `a * b + c` against node. | `kiln-native-toolchain/src/lib.rs` | `nbody`, `mandelbrot` checksums unchanged, and a probe that would differ under FMA added to `conformance/native` |
| 0.2 | Portable release target. `-march=native` makes a `kiln build` binary fail on an older CPU than the one that built it. Default to a portable baseline; native is opt-in. | `kiln-native-toolchain/src/lib.rs` | a release binary built here runs on the CI machine; `--march=native` documented as a flag |
| 0.3 | Finish NaN-boxing at the seam: `kiln_body_fn`, the `op_call` family, `read_property` / `write_property`, `kiln_object.slots`, array elements take `kiln_val`. Boundary conversion only at the public ABI. | `prelude_types.h`, `prelude_core.c`, `lib.rs` | `fib` below 92 ms and `class-poly` below 50 ms (their pre-`cf93bb61` numbers); zero `kiln_val_to_boundary` in an emitted body that makes no host call |
| 0.4 | Definite-assignment TDZ elimination. `dead_zone_slots` marks a slot guarded forever; replace with a dominance check per read. | `kiln-backend-c/src/lib.rs` | no `check_tdz` in the emitted C for any benchmark; Test262 TDZ chunks unchanged |
| 0.5 | Refusal diagnostics only on paths that can refuse. A typed arithmetic fast path has no status. | `lib.rs` | emitted C for `loop` under half its current line count |
| 0.6 | Real programs in the gate. Time the thirdparty bundles through the harness and baseline them with the Test262 no-regression rule. | `scripts/thirdparty.sh`, `bench/results` | a slower postcss or zod fails the gate, naming the bundle |

## Phase 1: the graph, weeks, the structural step

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 1.1 | Value graph. Abstract-interpret each function's stack bytecode into a CFG of SSA values (Turboshaft-style, not sea of nodes). Every allocation site, call site and property site gets a stable id carried to the C. Handlers and generator resume points are explicit edges. | 0.3 | every function in the native corpus round-trips through the graph with identical output |
| 1.2 | Graph verifier: dominance, type consistency, every guard has its invariant row, every exit has a map. Runs in debug builds and in the gate. Lands with 1.1, before any optimization pass. | 1.1 | the verifier rejects a hand-corrupted graph in a test; the gate runs it over the native corpus |
| 1.3 | Emit C from the graph. Values are C locals; `double` or `int64_t` where typed. No `stack[]`. `locals[]` only for cells and generator frames. Arguments materialize in a local array at the call site only. | 1.2 | snapshots under `tests/snapshots` regenerated; `loop` under 120 ms |
| 1.4 | Delete the 28 `can_fuse_*` matchers. Rewrites become one rule table (ISLE-style), not matchers. | 1.3 | `lib.rs` has no `can_fuse_`; the rule table covers every case the snapshots did |
| 1.5 | Per-function summaries as queries: parameter and result types, pure-or-unknown, returned sites, can-throw, reads `arguments`. Part of the interface hash so an edit reruns only changed summaries. | 1.1, `kiln-db` | `bench/` warm-edit rows unchanged; summary invalidation covered by the memo oracle |
| 1.6 | Whole-program fixpoint over the linked graph. Closed-world flag: no direct `eval`, `new Function`, or out-of-graph dynamic import (`function_contains_direct_eval` exists). Immutable bindings from `mutable_module_bindings`. | 1.5 | facts printed by `kiln build --explain` for a module: types, sites, immutable bindings, closed or open |
| 1.7 | Facts baseline. Snapshot the inferred types, sites and closed-world verdict per benchmark and per thirdparty bundle, the way `test262-baseline.json` snapshots pass counts. | 1.6 | a change to inference is a reviewable diff, chunk by chunk |

## Phase 2: speculation, calls, sites

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 2.1 | Versioning. For a region with a non-`unknown` guess, emit the guarded unboxed clone and the generic clone; a guard failure boxes registers and `goto`s the generic clone. Two clones, entry guard, exits at the header and before calls only. `KILN_SPECULATE=0` emits the generic clone alone; `KILN_SPECULATE=exit` fails every guard on first execution, then at random. | 1.3, 1.6 | `loop` ≤ 30 ms; `mandelbrot`, `nbody`, `collatz` within 1.2x of node; the native corpus passes under `=exit` |
| 2.2 | Differential fuzzing of the optimizer. A generator of small typed-ish programs in `fuzz/`, run under `KILN_SPECULATE=0`, `=1`, `=exit` and node, compared by output. | 2.1 | runs in CI nightly; a found divergence becomes a `conformance/differential` probe |
| 2.3 | Direct calls. A call to an immutable binding is a direct C call. Typed entry (`double f(double)`) beside the boxed entry; known-callee sites with typed arguments call it. Frame globals saved only when `publishes_frame`. | 2.1 | `fib` ≤ 5 ms; `frame` within 1.2x of node |
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
| Phase 5 | level | ahead | level | builtins at parity; startup unchanged at 2.6 ms; server lead structural |

Reference: node warm 25 / 21 / 0.48 ms; hand-emitted ceiling 28 / 3 / 0.5 ms.

## Do not

- Build a second backend. The graph is target-independent by design, not by schedule.
- Trust a TypeScript type. Guard it.
- Add a `can_fuse_*`. Add a rule.
- Add a third clone, a heap integer, or remove a bounds check. Reopen the invariants first.
- Measure on the shared checkout, or report a number without its checksum.
- Land a speculative path without its off switch, its exit mode, and a differential run.

## Reading, in order

Serrano, "Of JavaScript AOT Compilation Performance" (ICFP 2021) and "Property Caches Revisited"
(CC 2019); Chevalier-Boisvert and Feeley on lazy basic block versioning (2015, Higgs 2016), for
where a region should split even though this plan caps versions at two; Hackett and Guo, "Fast
and Precise Hybrid Type Inference for JavaScript" (PLDI 2012); Static Hermes design notes; Wimmer
et al., "Initialize Once, Start Fast" (VEE 2019); Stadler et al., partial escape analysis (CGO
2014); weval; V8's Turboshaft rationale; Cranelift's ISLE; Fuzzilli, for the shape of 2.2.
