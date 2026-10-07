# 10. The parallel family

Status: resolved (tier 1 scope; tiers 2-4 extend the lowering, see Comments)
Blocked by: 08 (for the allocating lowerings only)

Add, on `Array.prototype` and `%TypedArray%.prototype`, marked as kiln extensions (ADR 0068), each
with the sequential method's semantics exactly:

| method | identical to | note |
| --- | --- | --- |
| `parallelFilter` | `filter` | |
| `parallelFlatMap` | `flatMap` | arrays only |
| `parallelSort` | `sort` | stable; for a comparator that is consistent, the order is exactly `sort`'s. For one that is not, `sort`'s order is implementation-defined, so the call runs sequentially unless the comparator is proven consistent |
| `parallelReduce(fn, combine, initial)` | none | no sequential equivalent: the tree over the input is fixed by length alone, and the interpreter applies the same tree, so the answer is deterministic in both engines (ADR 0069) |

Each is TypeScript in `kiln-builtins` over the parallel primitive (ADR 0065), with the sequential
method as its fallback.

## Acceptance

- Each method's probes in the 02 harness, at every parallelism level, identical to the sequential
  method (or, for `parallelReduce`, identical across engines and core counts).

## Comments

2026-09-27, branch `work/parallel-family`.

**Sequential definitions, once, in TypeScript.** `kiln-builtins/src/parallel.ts` (in the common
bootstrap right after `typedarray.ts`, returns at once unless `kilnExtensions`) installs
`parallelFilter`, `parallelFlatMap`, `parallelSort`, `parallelReduce` on `Array.prototype` and
`parallelFilter`, `parallelSort`, `parallelReduce` on `%TypedArray%.prototype`, non-enumerable,
as method definitions (not constructors; lengths 1, 1, 1, 3). Filter, flatMap and sort call the
intrinsics captured at bootstrap (`Array.prototype.filter` etc., `%TypedArray%.prototype.filter`,
`.sort`), so results, exceptions and side effects are the sequential method's by construction.
`parallelReduce(callback, combine, initial)` is written out once: leaves of 256 indices
(`kiln_builtin_surface::PARALLEL_REDUCE_LEAF`; a `kiln-builtins` test holds the TS constant to
it), each folded left to right from `initial` (`callback(acc, O[k], k, O)`, holes skipped), then
combined pairwise level by level, an odd last one carried up; no input answers `initial`, one
leaf never calls `combine`. No method ids were added: no C prelude built-in, no `PROTOTYPE_SURFACE`
row.

**Extension mark.** Seven new `KILN_EXTENSIONS` rows. `KilnExtension` gained `installed_by:
ExtensionInstaller::{Description, Bootstrap}`; the const check now requires a `PROTOTYPE_SURFACE`
row for a `Description` row on a `PrototypeId` owner and forbids one for a `Bootstrap` row. The
`kiln-builtins` test (`bootstrap_extensions_match_the_description`) holds `defineExtension` /
`defineTypedArrayExtension` / `defineArrayExtension` calls to the bootstrap rows in both
directions. `KILN_EXTENSIONS=0` removes them in both engines (`kiln-extensions-off.ts` extended).

**Native runtime: `Array.prototype` store members were unreachable.** A member defined on
`Array.prototype` (bootstrap or a program's polyfill -- `Array.prototype.foo = f; [].foo()` was
`undefined` natively, on main too) now answers from an instance read (`read.c`), `in`
(`enumeration.c`), `getOwnPropertyNames` of a table-driven built-in (`keys.c`,
`kiln_c99_builtin_store_keys`, both branches) and `getOwnPropertyDescriptor`/`hasOwnProperty`
(`definitions.c`). The bootstrap's writes are sealed by the launcher's
`kiln_c99_builtin_store_epoch = 0`, so speculation is unaffected; a program that replaces a family
method bumps the epoch and every pooled lowering (all check it) stands down.

**Native lowering, one machinery.** `parallel::ParallelMethod` names the family and, per argument
count, which arguments are callbacks and what arity each may declare
(`callbacks(argc)`); `direct.rs` (full plan, `callback_targets`, generic-only
`plan_parallel_sites`) and `call_method.rs::parallel_site` read it, so verdicts, clones, typed
entries, `--explain-parallel` records (now carrying `method=`) and issue 13/14 generic-only units
and worker units all cover the family. `parallel_verdict` admits up to 3 parameters; arity is a
site rule now. New reasons: `argument-shape`, `comparator-unproven`.
- `parallelFilter`: `_filt`/`_filt_typed` workers write a keep mask over `kiln_parallel_map`;
  the realm thread counts and copies the kept elements in index order. A predicate whose returns
  are numbers and booleans qualifies (`ParallelShape::boolean_result`,
  `parallel::parallel_result`): its `_p` clone answers boxed, and it gets only the filter workers.
- `parallelFlatMap` (arrays): a numeric callback's answer is never an array, so it is
  `parallelMap`'s array lowering.
- `parallelReduce`: `_fold`/`_fold_typed` workers fold whole leaves (the pool's range is a range
  of leaves), then one pooled `_comb` call per tree level (`combine(r[2k], r[2k+1])`), odd one
  carried -- the TS tree exactly. Both callbacks must qualify and `initial` must be a number.
- `parallelSort`: new provider export `kiln_parallel_sort(data, count, kind, order)` (ABI 43 -> 44,
  116 -> 117 exports): Rayon `par_sort_by` (stable) at >= 10,000 elements, else `sort_by`, in
  place. Orders: the typed-array default (`-0` before `+0`, NaN last), or a comparator proven to
  be `(a, b) => a - b` / `b - a` (`parallel::comparator_order`: every reachable return is the
  difference of the two parameters); under a comparator a NaN makes the provider decline and
  `sort` answers. Arrays run only with a proven comparator (their default order is strings).
- `set_parallel` keeps the verdict and narrows only `parallel_safe` (a comparator needs its proven
  order, not a clone). Liveness demands the parallel provider for every family name.

**Two pre-existing native miscompiles the probes hit, fixed.** (1) `is_proven_array` took
`x.map/filter/slice/concat(...)` as an array whatever `x` was, so `view.slice().sort()` ran the
array `sort` over a typed array's block and left it unsorted; now only when the receiver is
itself proven an array. (2) A property site that had cached an array's `length`
(`cached_tag == KILN_VALUE_ARRAY`, in `boxed.c` and the compact `KU_READ_PROPERTY` macro) read
`kiln_array.count` off any buffer-prefixed value: `p.length` on a typed array read 0 in a
generic-only release module once the site had seen an array. Both reproduced on 050c6c39.

**Verified** (release `kiln`, full log `target/pfamily-logs/native.log`):
- `mbx test --offline`: `kiln-backend-c` 187 + 36 passed (ABI snapshot updated for
  `kiln_parallel_sort` and the keys helper; new `the_family_sites_are_decided_per_method`,
  `a_comparator_is_proven_consistent_only_as_a_difference_of_its_parameters`,
  `the_family_names_its_callbacks_by_position_and_arity`), `kiln-builtin-surface` 16,
  `kiln-builtins` 97, `kiln-native-provider-contract` 11, `kiln-liveness` 5,
  `kiln-native-provider-ffi --all-features` 52 (new `sort_orders_match_the_sequential_comparators`).
- `scripts/native.sh`: 253 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged. New
  probes `parallel-filter.js`, `parallel-flatmap.js`, `parallel-reduce.js`, `parallel-sort.js`,
  `parallel-family-refused.js`, each `profile=native-release parallelism=1,2,max`, comparing
  against `filter`/`flatMap`/`sort` with `Object.is` (and `parallelReduce` against its tree
  written out again, and against `reduce` where the combine is exact); by hand also at 1, 2 and 16
  threads. `mise run native-stress` not run.

**Timing**, 1M elements, one native-release binary, best of 7, 16-thread Ryzen 7 5800H, load ~2
(ms; "par @1" is the same binary with `KILN_PARALLELISM=1`):

| workload | sequential | parallel | par @1 |
| --- | --- | --- | --- |
| filter `x > 0` (array) | 19.6 | 5.6 | 4.8 |
| filter heavy (sin/cos/sqrt) | 219.8 | 7.0 | 33.3 |
| filter typed `Math.sin(x) > 0` | 359.1 | 5.9 | 12.9 |
| flatMap `x * 2 + 1` | 54.2 | 0.9 | 3.3 |
| flatMap heavy | 187.0 | 1.6 | 11.6 |
| reduce `a + x` | 16.7 | 0.15 | 0.82 |
| reduce heavy | 156.6 | 1.45 | 9.5 |
| reduce typed `a + sin(x)` | 316.8 | 1.6 | 9.5 |
| sort Float64Array default | 2099 | 114 | 140 |
| sort array `(a, b) => a - b` | 540 | 5.2 | 17.5 |

Much of the gap at one thread is the sequential methods' own native lowering (a generic call per
element; typed `sort` is TypeScript), not the pool.

**Limits of this scope / what tiers 2-4 extend.** A combine held in a captured `const` (`const add
= (a, b) => a + b; ... () => xs.parallelReduce(f, add, 0)`) is `callback-unknown`: tier 1 resolves
captured numbers, not captured functions. Tier 2 (read the heap) lets filter/reduce callbacks read
properties and arrays and `parallelSort` recognise key comparators (`(a, b) => a.k - b.k`); tier 3
(worker arenas) is what `parallelFlatMap` needs to return arrays and filter needs to keep objects;
tier 4 (pure calls) admits calls in callbacks and would let a comparator be *proven* consistent
beyond the two difference shapes. The `argc == 0` sort site is located by counting
`.parallelSort()` in the enclosing source (no instruction spans).

2026-09-27, branch `work/parallel-tier2` (issue 07). Tier 2 extends every member: `parallelMap`,
`parallelFlatMap` and `parallelFilter` over arrays of anything (holes kept or skipped as the
sequential methods do) and over views; `parallelReduce` per callback (a heap-reading fold with a
numeric combine, or the reverse); and `parallelSort` under a key comparator, `(a, b) => a.k - b.k`
or its reverse, which the pool sorts by key without calling the comparator. `parallelFilter` keeps
objects now (the predicate only reads; the realm thread copies the kept elements). Details and
the fallback argument are in issue 07's Comments.

2026-09-27, branch `work/parallel-tier4` (issue 09). Tier 4 extends every member: a callback of
`parallelMap`, `parallelFilter` or `parallelFlatMap` may call the built-ins marked pure and the
program's own proven functions (building strings and arrays in the arenas);
`parallelReduce`'s callbacks may call the ones that answer numbers without allocating
(`(a, l) => a + parseFloat(l)` runs its leaves on the pool); and a `parallelSort` key comparator may
call a pure callable global on each side (`(a, b) => parseInt(a) - parseInt(b)`). Details in issue
09's Comments.
