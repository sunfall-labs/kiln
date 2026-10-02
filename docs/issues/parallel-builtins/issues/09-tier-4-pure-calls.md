# 09. Tier 4: pure calls

Status: resolved
Blocked by: 07 (08 for calls that allocate)

1. **Pure built-ins.** Add a "pure" mark to the member description for built-ins with no side
   effects and no user-code callbacks for plain-data arguments: `Math`, string methods, number
   formatting (`toFixed`, `toString`), `JSON.stringify` of plain data, `Array.prototype.slice`,
   and so on. A parallel callback may call a marked built-in.
2. **Pure user functions.** Extend the proof interprocedurally: a callback may call a user function
   whose own graph satisfies the same rules, to a bounded depth, with recursion allowed when every
   function in the cycle is proven.

## Acceptance

- `lines.parallelMap(l => l.trim().split(",").map(Number))` runs in parallel.
- A call to an unmarked built-in or an unproven function keeps the call sequential, naming it.

## Comments

### 2026-09-27 -- tier 4, branch `work/parallel-tier4`

**The pure mark** (`kiln_builtin_surface::PURE_MEMBERS`, beside `KILN_EXTENSIONS`). A row per
member -- owner as the inventory spells it (`"globalThis"` for a callable global), key, what it
answers (`PureAnswer`: a string or array answer needs an arena), and the argument that is a
callback the proof must also cover (`map`'s). Marked: the 31 `Math` functions tier 1 already
lists (a test holds `PARALLEL_MATH` to the rows; `Math.random` is refused by a const assertion);
`Number`, `String`, `Boolean`, `parseInt`, `parseFloat` called as functions; `Number.isInteger`,
`isSafeInteger`, `isNaN`, `isFinite`, `parseInt`, `parseFloat`; `JSON.stringify`; on
`String.prototype` `at`, `charAt`, `charCodeAt`, `codePointAt`, `endsWith`, `includes`, `indexOf`,
`lastIndexOf`, `padEnd`, `padStart`, `repeat`, `slice`, `split`, `startsWith`, `substring`,
`toLowerCase`, `toString`, `toUpperCase`, `trim`, `trimEnd`, `trimStart`; `Number.prototype.toFixed`
and `toString`; on `Array.prototype` `every`, `filter`, `includes`, `indexOf`, `join`, `map`,
`slice`, `some`. Every row has a worker path (`parallel_heap::pure_path`), and a test holds the
two to each other in both directions and checks each C function exists in `boxed.c`
(`every_pure_member_has_a_worker_path_and_every_path_is_marked`).

**Worker paths** (`runtime/boxed.c`, "Parallel built-ins, tier 4"). `kiln_c99_par_m_<key>` per
method, dispatched on the receiver's kind (a string, a number, or an array with no named
properties); `kiln_c99_par_f_<name>` per static; `map`/`filter`/`some`/`every` are loops emitted in
the clone, calling the callback's clone (or a pure global) per element, holes skipped as the
sequential methods skip them. Each answers exactly what its sequential definition answers or
declines: an object receiver (its own `trim` could be anyone's), an object/`RegExp` separator, a
string or object argument whose conversion would parse or call `valueOf`, a `RangeError` argument
(`toFixed(101)`, `repeat(-1)`), a non-ASCII case map, a result array past 64 elements, a `toJSON`
anywhere on a stringified object's chain, a cycle or a BigInt. Strings are built flat in the
worker's arena; nothing interns (a `toJSON` lookup uses `kiln_c99_interned_key_in`). Two
sequential definitions were split so both paths share their digits rather than restate them:
`kiln_c99_fixed_spelling` (`toFixed`, `numbers.c`) and `kiln_c99_parse_int_bytes`/`_float_bytes`
(`dispatch_part_a.c`). `Number(s)` handles Latin1 and UTF-16 text without allocating (white space
trimmed as the provider trims it; any other non-ASCII unit is NaN). These are worker paths of
existing members, not new built-ins: the `prelude_core.c` freeze is about where a member's
definition lives, and a pool thread cannot run TypeScript.

**Pure user functions** (`parallel::heap_verdict_in`, a `Proof` walk; `CallGraph` from
`direct.rs`). A call whose callee the unit knows -- a known closure, a closure written in the
callback, or a captured `const f = ...` whose one real store is one closure
(`Known::captured_closures`, new) -- is followed into that function's graph and held to the same
rules (a `return` without a value allowed there), up to `PARALLEL_CALL_DEPTH` (8) functions deep
and `PARALLEL_CALLEES` (32) functions in all. A call back into a function already being proven is
admitted: the walk proves every function of the cycle, so recursion is allowed exactly when the
whole cycle is. Each callee gets a clone of its own per callback (`{symbol}_fn_{cb}_hc{f}`,
`emit_heap_callees`) with its worker cache entries after the callback's, taking the captures its
closure holds (`ups`, read on the worker from the closure the call found -- bindings written once,
checked for the dead zone) and a depth (a pool thread declines past `KILN_C99_PAR_CALL_DEPTH`,
128, and the sequential run finishes or overflows). A call through a closure value first compares
the closure's body with the proto's (`env->bodies`, filled on the realm thread), so a resolution
that is wrong at run time declines rather than calls the wrong clone. A closure written in the
callback is never made: its uses are held to "only called" (or a pure array method's callback),
and its captures are handed down from the callback's.

**Reasons and `--explain-parallel`.** `parallel (tier 4: pure calls)` (marker `tier=4`, `builds=1`
when it also builds). New codes: `calls-impure` ("calls Math.random, which is not marked pure"),
`calls-unproven` naming the chain and the rule (``calls `outer` -> `inner` -> `bump`
(captures-reassigned: calls), which is not proven pure``), `calls-unknown` (a value the compiler
cannot identify as one function, named by its binding), `calls-too-deep`. Tier 1's `calls` stays
for tier 1's own refusals.

**A patched built-in** (`String.prototype.trim = ...`) stands every parallel site down through the
realm's modified-built-ins record (`kiln_c99_builtins_unmodified()`, already every pooled
lowering's guard): verified with `KILN_PARALLEL_TRACE=1` (no join). What the sequential run then
answers is issue 30's problem: natively a replaced prototype method is never called at all, on
main too, so `parallel-pure-refused.js` compares parallel with sequential only.

**Acceptance.**
- `lines.parallelMap(l => l.trim().split(",").map(Number))` runs in parallel: `parse-csv.mjs:13:17
  parallelMap  parallel (tier 4: pure calls)`, and `parallel-pure-strings.js`'s `csv` equals `map`.
- An unmarked built-in or an unproven function keeps the call sequential, naming it:
  `parallel-pure-refused.js` (a helper that writes, the write two calls down, `push`, a reassigned
  helper, a write to a captured array, `Math.random`), each with the effects of `map` in order.

**Probes** (`conformance/native`, `profile=native-release parallelism=1,2,max`, each comparing the
parallel method with the sequential one and both engines with each other): `parallel-pure-strings`,
`-numbers`, `-arrays`, `-json`, `-helpers` (helpers calling helpers, captured constants, a closure
written in the callback, `fact`, mutual `isEven`/`isOdd`, `fib`, a recursion past the pool's depth
near the end), `-refused`, and `-stress` (no thread directive, allocation-free checks, for
`mise run native-stress`).

**Found here, filed:** issue 30 -- natively a replaced prototype method is never called; `split` of
a UTF-16 string ignores a limit below its piece count; `parseInt`/`parseFloat` of a UTF-16 string
fail with "out of memory"; a 5,000-frame recursion overflows. All sequential, all on main; the
probes keep to inputs where the sequential native answer is right.

**Also here.** `parallelSort`'s key comparator may call a pure callable global on each side
(`(a, b) => parseInt(a) - parseInt(b)`): the pool computes each key once and sorts by it
(`parallel-pure-numbers.js`, "sort by a parsed key").

**Verified** (release `kiln` of this branch; logs under `target/tier4-logs/`):
- `mbx test -p kiln-backend-c --offline`: 215 + 36 passed, 0 failed (new in `parallel.rs`:
  `tier_4_calls_built_ins_marked_pure`, `tier_4_names_a_built_in_that_is_not_marked_pure`,
  `tier_4_follows_calls_into_the_program`, `tier_4_admits_recursion_when_the_cycle_is_proven_and_bounds_depth`,
  `tier_4_calls_a_closure_written_in_the_callback`, `every_pure_member_has_a_worker_path_and_every_path_is_marked`,
  `tier_4_sites_say_so`, `tier_4_a_key_comparator_may_call_a_pure_global`; in `parallel_tests.rs`
  `tier_4_pure_calls_run_in_the_pool_and_match_the_sequential_methods`, a threaded fixture where
  both sites take the pool). Prelude size band widened (+~1,300 lines) and the ABI surface snapshot
  updated. `kiln-builtin-surface`: 18 passed (new `every_pure_static_is_a_surface_member`).
- `scripts/native.sh`: 303 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged, the seven
  new probes at `parallelism=1,2,max` included; after the key-comparator change, the sort probes
  and `parallel-pure-numbers.js` again through `native.sh` (`KILN_NATIVE_DIR`): 5 matched.
- Stress, by hand (`native.sh` skips `parallelism=` probes under stress): `parallel-pure-stress.js`
  at `KILN_GC_STRESS=1` at 1 and 4 threads (4 joins at 4 threads; at 1 the array branch stands down,
  issue 22), and the six larger probes at `KILN_GC_STRESS=64` (inputs cut to 1,501) at 1 and 4
  threads -- all equal to the interpreter.

**Bench** (`bench/parallel/run.sh --quick --only parse-csv`, 1M lines, ms per call, one round,
16-thread Ryzen 7 5800H, load 1.5-5; before = release `kiln` at bc06ed29):

| workload | node | bun | kiln seq | before @1 / @4 / @16 | after @1 / @4 / @16 |
|---|---|---|---|---|---|
| `parse-csv` `l.trim().split(",").map(Number)` | 387-390 | 222-229 | 636-652 | 625 / 627 / 626 | 624 / 146 / 100 |

@16 is 6.4x kiln's `map` and 3.9x node's; the sequential column is still 1.6x node's, which is
the next thing to fix (every line allocates a trimmed copy, an array and three substrings).

**Risks / left.**
- A worker path is a second implementation of a member's answer. Where the two could drift, the
  digits are shared (`toFixed`, `parseInt`, `parseFloat`); the rest are held to the sequential
  method by the probes, and decline rather than guess at every edge they do not cover.
- Arrays a pure method builds on a pool thread are capped at 64 elements (the in-block limit);
  a longer `split` or `map` declines and the call runs sequentially from the start.
- Worker recursion is bounded at 128 clone frames; a callee with a very large body could need more
  stack than a pool thread has before that bound (none seen).
- A call through a captured closure is resolved heuristically at compile time
  (`captured_closures`) and checked per call at run time (the body comparison); a wrong guess costs
  a sequential run, never a wrong answer.
- Natively a replaced prototype method is never called (issue 30); the parallel site stands down,
  but the sequential answer is still the built-in's.
