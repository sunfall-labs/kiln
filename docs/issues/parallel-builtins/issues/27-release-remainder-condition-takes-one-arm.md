# 27. At `native-release`, `i % 2 ? a : b` in a two-parameter callback always takes `a`

Status: resolved

Found by issue 08 (tier 3) while writing `parallel-alloc-flatmap.js`; still present after issue 20
(`561fbf41`). A callback `(x, i) => i % 2 ? shared : [x]` answers `shared` for every element at
`native-release`, sequentially (`KILN_PARALLELISM=1`, so no pool is involved):

```js
const shared = [1, , 3];
const xs = [10, 20, 30, 40];
console.log(JSON.stringify(xs.map((x, i) => i % 2 ? shared : [x])));
// interpreter, native-dev: [[10],[1,null,3],[30],[1,null,3]]
// native-release:          [[1,null,3],[1,null,3],[1,null,3],[1,null,3]]
```

`i % 2 === 1 ? ...` and `(i & 1) ? ...` are right, so the suspect is the truthiness of an integer
remainder's twin (the `Int` proof issue 20 narrowed) where it is used directly as a condition.
The tier-3 probes use `(i & 1)` meanwhile.

## Acceptance

- The example matches the interpreter at `native-release`, and a probe pins it.

## Comments

2026-09-27, branch `work/miscompile-27`.

**Root cause: a dropped box, not remainder typing.** Neither the `Int` proof (issue 20) nor the
parallel clones: the fault is in `boxed_uses` (`emit_graph/body.rs`), which decides which values
some operation reads *boxed* so that a proven number nobody reads boxed keeps only its twin. For a
`Branch` terminator it exempted the condition when it was a `Binary` of two proven numbers whose
operator satisfies `twin_only` -- but `twin_only` lists the arithmetic operators too (`+ - * / **
%`), while `emit_terminator` inlines only *comparisons* on twins. An arithmetic condition therefore
lost its box, and the branch still wrote `kiln_c99_val_truthy(vN)` over a `vN` nothing assigned:

```c
d2 = (... d0 % d1 ...);          /* the remainder, twin only */
if (kiln_c99_val_truthy(v2)) {  /* v2 never written */
```

Every release fast clone and typed entry had it: `i % 2 ? a : b`, `n - 1 ? ..`, `if (a / b)`,
`while (n % 7)` (hangs), and every truthiness-testing callback (`filter((x, i) => i % 2)`,
`findIndex`, `some`, `reduce`, `forEach` with an `if`). `i % 2 === 1` and `(i & 1)` were right
because a comparison is inlined and a bitwise op on ints is not `twin_only`. Sequential,
`KILN_PARALLELISM=1`, and the pooled lowerings alike: the callback's typed clone is the same body.

**Fix** (`boxed_uses`). The branch exemption asks `is_comparison(op)` -- exactly what the branch
inlines -- so an arithmetic condition keeps its box. **The class** turned up one more member of
the same function: `+x` / `ToNumeric` of a proven number was exempt as twin-only, but both
emitters write its box as a copy, `vU = vX`; `const t = a * 2; return [+t]` answered `[0]`. It is
now treated like a phi (it reads `x` boxed exactly when its result is read boxed). The other
exemptions were checked against their emitters (negate/increment read `d`; `~` on ints reads `i`;
pure `Math`/`Number` predicates, int keys, typed returns read twins) and probed: they are right.

**Tests.** `kiln-backend-c`: `executable_branch_on_a_number_remainder_reads_a_written_box`
(`f(0) * 100 + f(1)` for `i % 2 ? 7 : 11`; fails without the fix) and
`executable_unary_plus_of_a_product_copies_a_written_box` (`g([2.5][0])` through the boxed entry's
fast clone; fails without the fix). Probe `conformance/native/release-arithmetic-condition-box.js`
(`profile=native-release parallelism=1,2,max`): the issue's example, every arithmetic operator as
a condition, loop conditions, every truthiness callback with an index parameter, `+x`, and
`parallelFlatMap`/`parallelMap`/`parallelFilter` with `i % k` conditions. It diverges on base
`59bbdfe9` and matches on the branch.

**Cost, for later.** A branch on an arithmetic number now boxes it and asks `ku_truthy`, whose
double case is a non-inlined call: `for (..) if (i % 3) c++` over 3e8 is ~1.2-1.7 s at release
against ~0.45 s for `(i % 3) !== 0` (noisy machine). There is no faster *correct* baseline -- the
old code was fast by being wrong -- but testing the twin (`d != 0 && d == d`, `i != 0`) for a
number condition would recover it; that needs the condition seeded as a live twin in both clones
(the `Return` of a typed number is the precedent), so it is left as a follow-up rather than done
here.

**Results** (branch's final compiler, release CLI): `kiln-backend-c` 208 + 36 passed, 0 failed;
`scripts/native.sh` 290 matched, 1 refused (`shadow-realm`), 0 diverged; the `native-release`
corpus pass (every `conformance/native` probe forced to `native-release`, `timeout -k 10 900` per
run, no stop at the first broken build) **290 matched, 0 diverged, 0 broken, 1 refused**
(`shadow-realm`); `kiln test conformance/regress --node` 39/39.

### 2026-09-27 -- numeric branch twin follow-up

`emit_graph/body.rs` now treats a proven numeric branch condition as a live twin and tests it
directly: `i != 0` for an integer, or `d != 0.0 && d == d` for a double (so both zero signs and
NaN are false). Boxed-use analysis and branch emission use the same comparison predicate. The
generated release body for `if (i % 3)` now branches on its written double twin without a box or
`ku_truthy` call. The backend crate passed 217 tests plus 36 emitted-C tests; the native corpus
matched 312 probes with 1 existing ShadowRealm refusal and 0 divergences.

Two `native-release` binaries each counted 300 million loop iterations, one with `if (i % 3)` and
one with `if (i % 3 !== 0)`. Both printed `200000000`. Seven interleaved timed runs after one
warm-up had medians of 0.25809 s and 0.25832 s respectively; the ranges were 0.25545–0.26277 s
and 0.25755–0.28269 s. System load was about 5.85 throughout, so this is a focused equivalence
check under that load, not a broad performance baseline. Raw samples are retained locally at
`~/.cache/kiln/arith-branch-2026-09-27/perf.json`.
