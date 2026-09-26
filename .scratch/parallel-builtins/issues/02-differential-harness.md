# 02. The parallel/sequential differential harness

Status: resolved

Every tier is held to one property: the parallel result is exactly the sequential one. Build the
harness before extending any tier.

- A corpus, `conformance/native/parallel-*.ts`, of callbacks at every tier (qualifying and not),
  each run through `parallelMap` and through `map`, printing both and asserting they are identical,
  including which exception is thrown when several elements throw.
- A property test that generates callbacks and inputs from the qualifying grammar of each tier and
  compares parallel and sequential results natively (`proptest`, as ADR 0060 already uses).
- Each probe also runs under `KILN_GC_STRESS` (`mise run native-stress`), since tiers 2 and 3
  change what the collector must tolerate.
- Each probe runs at `KILN_PARALLELISM=1`, `2` and the machine's core count, and must print the
  same bytes at each.

## Acceptance

- `scripts/native.sh` runs the parallel probes; a mismatch fails it.
- The property test runs in `mbx test -p kiln-backend-c`.

## Comments

2026-09-26, branch `alee/parallel-tier1`.

**Corpus.** `conformance/native/parallel-*.js`, each running every callback through
`parallelMap` and `map`, comparing element by element with `Object.is` and `in` (so `-0`, `NaN`
and holes count), and throwing on a difference (so a disagreement fails the interpreter outright,
and a native-only one is a divergence):

- tier 0, qualifying: `parallel-arith`, `parallel-math` (every listed `Math` function, small and
  empty arrays), `parallel-edges` (both zeros, NaN, infinities, denormals, int32/uint32 edges,
  2^53+1, halves, across chunk boundaries);
- tier 1: `parallel-capture-const`, `parallel-capture-guard` (captures a run-time guard turns
  away: string, boolean, undefined, null, dead zone), `parallel-capture-scope` (parameters,
  two-level closures);
- `parallel-typed-arrays`: all nine kinds, at pooled and inline sizes, over every conversion edge;
- non-qualifying: `parallel-refused-release` (release-specialized), `parallel-refused` and
  `parallel-throws` (large, generic): property reads, allocation, strings, calls, `this`,
  `arguments`, a thisArg, side-effect order, holes, mixed and sparse arrays, a growing and
  shrinking receiver, and which exception is thrown -- and after how many calls -- when several
  elements throw.

**Two constraints the harness ran into, both now written down.** The development profile (the
default of `kiln run --native`) emits generic bodies only, and so does a *release* module past 16
functions or 512 instructions (`kiln-native`'s `MAX_RELEASE_SPECIALIZED_MODULE_*`): neither has
any parallel lowering. So `scripts/native.sh` learned a directive, `// kiln-native:
profile=native-release parallelism=1,2,max`, which builds the probe with `--profile
native-release` and runs the binary once per `KILN_PARALLELISM` value (`max` = `nproc`); every
run must print the interpreter's bytes. The probes stay under the size limit, and
`kiln build --profile native-release --explain-parallel` (issue 03) confirms which sites are
parallel in each (e.g. `parallel-math`: 7/7, `parallel-edges`: 7/7, `parallel-capture-const`: 6/7,
the seventh the reassigned `let`). Documented in `conformance/native/README.md`.
`KILN_GC_STRESS` flows through the same loop, so `mise run native-stress` covers the probes; it
was not run here.

**Property test.** `crates/kiln-backend-c/src/parallel_tests.rs`,
`parallel_map_is_map_bit_for_bit` (proptest, now a dev-dependency of `kiln-backend-c`): callbacks
generated from the qualifying grammar (element, index, two captured bindings written once,
awkward literals, twelve binary operators, negate, `~`, fifteen one-argument and five
two-argument `Math` functions), each also as `1 / (callback)` so a `-0` shows in the text the
scalar ABI returns, run through `parallelMap` and `map` in one compiled program over inputs with
NaN, -0, infinities and denormals. The pool is a test fixture that splits every call across four
POSIX threads whatever its size, and counts its calls: the test fails if any site was not lowered
parallel (from the site records of issue 03) or fell back at run time. 3 cases × 8–12 callbacks
per `mbx test` (≈20 s); run once with `PROPTEST_CASES=16` while developing.

**What it found -- all fixed here, none of them in the parallel path itself:**

1. The typed two-argument `Math` lowering (`min`, `max`, `pow`, `atan2`, `hypot`, ...) declared
   its operands as `double d0, d1`, shadowing the SSA values `d0`/`d1`: `Math.min(-i, x)` read
   its own first operand as its second. Plain `map` answered `-i` for every element.
   Regression test `math_min_operands_are_not_shadowed_by_their_temporaries`.
2. `map`'s typed loop with a boxed result set the result's length only after the loop, so a
   collection mid-loop swept every string stored so far (a second `xs.map(x => x + s)` over 17,000
   elements had ~6,000 wrong elements).
3. `map`'s typed and generic loops, and `parallelMap`'s generic loop, assumed the callback cannot
   grow, shrink or reallocate the receiver: a stale element pointer, a write past the result's
   capacity, and no holes for indices gone by their visit. They now read the length once and each
   element at its visit (the pure typed loop is unchanged).
4. `parallelMap`'s generic lowering ignored the thisArg and passed the array as `this`.
5. The pooled array path named an undeclared `kiln_inheriting_new_target`: every release build
   with a qualifying `parallelMap` on an array failed to compile. It had never run.
6. The interpreter's `Math.round(-0)` answered `+0` (Node: `-0`); the native backend was right.

**Verified** (debug `kiln`, full log kept and grepped): `mbx test -p kiln-backend-c --offline`
205 + 36 passed, 0 failed; `mbx test -p kiln-cli --offline` all passed; `scripts/native.sh`:
232 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged. The parallel probes at `KILN_PARALLELISM=1`, `2` and the core count are part of
that run (the directive loop); on their own: 12 matched, 0 diverged.
