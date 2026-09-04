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
  off (`KILN_SPECULATE=0`, sibling of `KILN_KEYS_CHECK`), and the differential corpus runs both.
- Nothing new is implemented twice. A new builtin goes into one source (Phase 5); until then it
  goes where the existing one lives.
- The C emitter is the only backend. No Cranelift, no Wasm target, no LLVM IR until C blocks a
  named optimization.

## Phase 0: corrections, days each, all parallel

| # | step | where | done when |
| --- | --- | --- | --- |
| 0.1 | Release profile uses `-ffp-contract=off`. FMA contraction changes the last bit of `a * b + c` against node. | `kiln-native-toolchain/src/lib.rs` | `nbody`, `mandelbrot` checksums unchanged, and a probe that would differ under FMA added to `conformance/native` |
| 0.2 | Finish NaN-boxing at the seam: `kiln_body_fn`, the `op_call` family, `read_property` / `write_property`, `kiln_object.slots`, array elements take `kiln_val`. Boundary conversion only at the public ABI. | `prelude_types.h`, `prelude_core.c`, `lib.rs` | `fib` below 92 ms and `class-poly` below 50 ms (their pre-`cf93bb61` numbers); zero `kiln_val_to_boundary` in an emitted body that makes no host call |
| 0.3 | Definite-assignment TDZ elimination. `dead_zone_slots` marks a slot guarded forever; replace with a dominance check per read. | `kiln-backend-c/src/lib.rs` | no `check_tdz` in the emitted C for any benchmark; Test262 TDZ chunks unchanged |
| 0.4 | Refusal diagnostics only on paths that can refuse. A typed arithmetic fast path has no status. | `lib.rs` | emitted C for `loop` under half its current line count |

## Phase 1: the graph, weeks, the structural step

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 1.1 | Value graph. Abstract-interpret each function's stack bytecode into a CFG of SSA values (Turboshaft-style, not sea of nodes). Every allocation site, call site and property site gets a stable id carried to the C. Handlers and generator resume points are explicit edges. | 0.2 | every function in the native corpus round-trips through the graph with identical output |
| 1.2 | Emit C from the graph. Values are C locals; `double` or `int64_t` where typed. No `stack[]`. `locals[]` only for cells and generator frames. Arguments materialize in a local array at the call site only. | 1.1 | snapshots under `tests/snapshots` regenerated; `loop` under 120 ms |
| 1.3 | Delete the 28 `can_fuse_*` matchers. Rewrites become one rule table (ISLE-style), not matchers. | 1.2 | `lib.rs` has no `can_fuse_`; the rule table covers every case the snapshots did |
| 1.4 | Per-function summaries as queries: parameter and result types, effects, returned sites, can-throw, reads `arguments`. Part of the interface hash so an edit reruns only changed summaries. | 1.1, `kiln-db` | `bench/` warm-edit rows unchanged; summary invalidation covered by the memo oracle |
| 1.5 | Whole-program fixpoint over the linked graph. Closed-world flag: no direct `eval`, `new Function`, or out-of-graph dynamic import (`function_contains_direct_eval` exists). Immutable bindings from `mutable_module_bindings`. | 1.4 | facts printed by `kiln build --explain` for a module: types, sites, immutable bindings, closed or open |

## Phase 2: speculation, calls, sites

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 2.1 | Versioning. For a region with a non-`unknown` guess, emit the guarded unboxed clone and the generic clone; a guard failure boxes registers and `goto`s the generic clone. Region boundaries follow lazy basic block versioning (Higgs): split where incoming types differ, cap versions per site. `KILN_SPECULATE=0` emits the generic clone alone. | 1.2, 1.5 | `loop` ≤ 30 ms, `mandelbrot`, `nbody`, `collatz` within 1.2x of node |
| 2.2 | Direct calls. A call to an immutable binding is a direct C call. Typed entry (`double f(double)`) beside the boxed entry; known-callee sites with typed arguments call it. Frame globals saved only when `publishes_frame`. | 2.1 | `fib` ≤ 5 ms; `frame` within 1.2x of node |
| 2.3 | Inline sites. PIC hit path emitted inline: shape compare, slot load. Prototype epoch: objects flagged when they become a `[[Prototype]]`, one realm counter bumped on a write to a flagged object, PIC entries record it. Intrinsics for `Math.*`, array `length` and `push`, string `length` and index, under a builtin-write epoch. Closed program: no epoch check. | 1.5, 2.2 | `class-poly` ≤ 1 ms; `collections`, `object-spread` within 2x of node |
| 2.4 | Integers. `int` in the lattice with overflow checks falling back to `double`; induction-variable ranges remove `%` integrality tests and array bounds checks. | 2.1 | `loop` level with node; `array-sort` bounds checks gone from the emitted C |

## Phase 3: type sources and profiles

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 3.1 | TypeScript annotations as guarded guesses at function entry. Never trusted, always guarded. Order the trusted annotations by Static Hermes's published results. | 2.1 | a typed `dist(): number` compiles to the same C as the inferred one |
| 3.2 | Type profile. `--profile native-profile` writes observed tags per site; the next build reads it as the guess for untyped code. | 2.1 | thirdparty bundles within 2x of node with a profile, no regression without |
| 3.3 | Clang PGO and post-link layout. The same training run emits a sampled profile for `-fprofile-use`; BOLT or Propeller when the toolchain has it. | 3.2 | measured on `bench/server` mixed: report the delta, keep it if positive |

## Phase 4: memory

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 4.1 | Escape analysis on the graph; scalar replacement of non-escaping objects (partial escape analysis). | 1.5 | `new Point` in a non-escaping loop allocates nothing; `object-spread` within 1.5x |
| 4.2 | Turn arenas. Object graphs proven dead by turn end bump-allocate in an arena reset at the turn boundary (ADR 0041 is the region boundary). | 4.1 | `bench/server` mixed: request path allocates nothing traced; rps and p99 reported against node and bun |
| 4.3 | Arrays proven numeric use unboxed `double[]` backing; strings expose length and bytes without a call. | 1.5 | `array-functional`, `string-ops` within 1.5x |

## Phase 5: one semantics

| # | step | depends on | done when |
| --- | --- | --- | --- |
| 5.1 | Freeze `prelude_core.c`. New builtins are TypeScript with intrinsics (design.md §5). | none | policy in `CLAUDE.md`; a builtin added in C is a review refusal |
| 5.2 | Partial evaluation of builtins. The graph inlines and specializes a TypeScript builtin at each call site (weval's first Futamura projection, applied to the graph). Migrate hot families one at a time with a measurement each: array, string, JSON, Map/Set, iterator. | 2.3, 4.3 | each migrated family within 1.2x of its C version, then the C version deleted |
| 5.3 | One description of each instruction's semantics generates the interpreter dispatch and the C emission templates (Deegen). Ends the two-engine implementation of the instruction set. | 1.2 | interpreter and native disagree on nothing the differential corpus can express; the corpus becomes optimizer-vs-interpreter |
| 5.4 | Regex literals lowered to C matchers at build time; `regress` remains for dynamic patterns. | 1.2 | `regex` within 1.5x of node |
| 5.5 | Heap image. Pure top-level initialization runs at build time and ships as data. | 1.5 | start time of a module-heavy program equals `exec` of an empty one |

## Targets

| after | `loop` | `fib` | `class-poly` | everything else |
| --- | --- | --- | --- | --- |
| Phase 0 | ~120 ms | < 92 ms | < 50 ms | no regressions |
| Phase 2 | ≤ 30 ms | ≤ 5 ms | ≤ 1 ms | class A and B within 1.5x of node |
| Phase 3 | level | ahead | level | typed real code ahead; bundles within 2x |
| Phase 5 | level | ahead | level | builtins at parity; startup unchanged at 2.6 ms; server lead structural |

Reference: node warm 25 / 21 / 0.48 ms; hand-emitted ceiling 28 / 3 / 0.5 ms.

## Do not

- Build a second backend. The graph is target-independent by design, not by schedule.
- Trust a TypeScript type. Guard it.
- Add a `can_fuse_*`. Add a rule.
- Measure on the shared checkout, or report a number without its checksum.
- Land a speculative path without its off switch and a differential run.

## Reading, in order

Serrano, "Of JavaScript AOT Compilation Performance" (ICFP 2021) and "Property Caches Revisited"
(CC 2019); Chevalier-Boisvert and Feeley on lazy basic block versioning (2015, Higgs 2016);
Hackett and Guo, "Fast and Precise Hybrid Type Inference for JavaScript" (PLDI 2012); Static
Hermes design notes; Wimmer et al., "Initialize Once, Start Fast" (VEE 2019); Stadler et al.,
partial escape analysis (CGO 2014); Deegen and LuaJIT Remake; weval; V8's Turboshaft rationale;
Cranelift's ISLE.
