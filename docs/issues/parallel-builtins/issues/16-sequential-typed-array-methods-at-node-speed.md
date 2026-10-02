# 16. Sequential typed-array methods at node's speed

Status: resolved

Found by the benchmarks (issue 11): natively, kiln's sequential typed-array methods are 10-45x
slower than node's: `map`, `filter`, `reduce` 400-560 ms per million elements, the default `sort`
5.3 s, `slice` about 200 ms. They are the baseline every parallel speedup is measured against and
what most numeric code calls. `%TypedArray%.prototype` is written in TypeScript
(`kiln-builtins/src/typedarray.ts`, ADR 0005/0060); find where the time goes natively (generic
calls per element, element access through generic property paths, the sort algorithm) and fix it at
the right layer: declared primitives for bulk element access (ADR 0065), direct typed callback calls
where the callback is known, and a proper numeric sort for the default comparator.

## Acceptance

- `map`, `filter`, `reduce`, `slice` and default `sort` on a 1M-element `Float64Array` within 2x of
  node natively; no interpreter regression; `scripts/native.sh` 0 diverged.

## Comments

2026-09-27, branch `work/sequential-fast-paths`.

**Profile** (perf, unstripped native-release build, 1M `Float64Array`). The bootstrap
(`@kiln/runtime-bootstrap.ts`) is emitted generic-only and compiled at `-O0` for size, so the
TypeScript methods pay, per element: `Function.prototype.call` through
`kiln_c99_invoke_method_dispatch` (`callback.call(thisArg, ...)`), a generic element read
(`read_element` -> `get_computed` -> `typed_element`) and write, and generic `unary`/`binary`
for `k++` and the comparisons -- `kiln_val_to_boundary` alone was 11%. Nothing dominated; the
whole path was generic. The default `sort` copied into an array and sorted it with a TypeScript
comparator closure (2.5-5 s). `join` appended to one string per element (quadratic: 5.2 s for
100k). `indexOf`, `fill`, `reverse` and `set` ran 55-100 ms on the element read/write alone.

**Design.**
- *Callback methods, at the call site* (`emit_graph/ops/callback_loops.rs`): `map`, `filter`
  and `reduce` on a typed array run a loop in the site's own specialized code, beside the array
  loops, calling the callback's typed entry directly when the site proves the callee (and it
  reaches nothing outside itself), else the closure body. A documented measured fast path over
  the TypeScript owner (ADR 0060): the guard is a Number-kind view, not a `Buffer`, attached, and
  `kiln_c99_builtin_store_epoch == 0` -- native views have no own properties or chosen
  prototypes, so the method, `ValidateTypedArray` and the species constructor are then the
  bootstrap's; every failed guard is the generic call into the TypeScript. The loop takes the
  TypeScript's steps in order (length once, element read at its visit and `undefined` once
  detached, `ToNumber` on the write).
- *Storage, as declared primitives* (ADR 0065, representation): `__kilnTypedArrayCopy(target,
  targetIndex, source, sourceIndex, count)` -- a same-type byte copy as if through a temporary,
  `false` (nothing written) for two types, a range past either length, or a detached buffer --
  under `slice`, `set`, `copyWithin`, `fill` (one element, then doubling copies) and `toSorted`;
  and `__kilnTypedArraySort(view)`, the default order in place. Method ids 537 and 538
  (`KILN_C99_METHOD_TYPED_ARRAY_COPY_RAW`, `_SORT_RAW`); `Native::TypedArrayCopyRaw`,
  `TypedArraySortRaw` in the interpreter.
- *One owner of the default order*: `kiln_number_core::typed_order` (new; allocation-free, the
  caller hands it key scratch) maps each element to a `u64` key whose unsigned order is the
  default order (`-0` before `+0`, every `NaN` last as the canonical quiet NaN; all twelve kinds
  including Float16 and the BigInt kinds) and sorts the keys. The interpreter calls it directly;
  native reaches it through a new provider export `kiln_typed_array_sort(data, count, kind)` in
  the `root` group (ABI 44 -> 45, 117 -> 118 exports), sequential on the calling thread; the
  parallel sort's `SORT_DEFAULT` comparator is now `typed_order::compare_default`.
- *TypeScript*: callbacks are called directly when there is no thisArg (`Call(f, undefined,
  ...)`, which does not read `Function.prototype.call` -- more correct, and half the cost
  natively); a comparator goes straight to `Array.prototype.sort` (its `SortCompare` is this one
  for elements that are never `undefined`); `join` collects pieces and joins once; the two
  copies of the default comparator in `sort`/`toSorted` are one `sortItems`, now only the
  fallback for a declined primitive.

**Timings**, native-release, 1M `Float64Array`, best of 7 (ms). A module within the release
profile's specialization limits (<= 16 functions, <= 512 instructions -- every
`bench/parallel` workload is one):

| method | node | before | after |
|---|---|---|---|
| `map(x => x * 2 + 1)` | 12.5 | 270 | 1.48 |
| `filter(x => x > 0)` | 21.0 | 289 | 4.62 |
| `reduce((a, x) => a + x, 0)` | 9.55 | 219 | 2.76 |
| `slice()` | 2.01 | 112 | 2.93 |
| `new Float64Array(xs).sort()` | 83.9 | 2,509 | 21.1 |

In a larger module (generic-only at `-O0`, so no site loops): `map` 250 -> 133, `filter` 273 ->
148, `reduce` 209 -> 83, `forEach`/`some`/`every`/`find` ~225 -> ~90, `slice` 2.96, `sort` 21.5,
`toSorted` 19.3, `set` 97 -> 3.0, `join` (100k) 5,220 -> 24, `fill` (incl. allocation) 57 -> 2.7,
comparator `sort` 1,967 -> 613. `bench/parallel/run.sh --quick` "kiln seq" column (noisy
machine): map-light 270 -> 4.56, map-heavy 307 -> 31.5, filter 523 -> 38.5, reduce-sum 673 ->
3.56, reduce-fold 268 -> 60.0, sort-typed 2,890 -> 43.5 (node in the same run 24.8, 73.0, 62.0,
21.2, 380, 141).

**Verified.** `scripts/native.sh`: 262 matched, 1 refused (`shadow-realm`, pre-existing), 0
diverged. New probes `typed-array-site-loops.js` (every Number kind, typed and boxed answers,
conversions), `typed-array-site-loops-edges.js` (detach mid-loop, throw, thisArg, writes during
the loop, BigInt), `typed-array-storage-primitives.js` (sort/toSorted/slice/copyWithin/set/fill/
join per kind, BigInt sort); interpreter output == node for all three; the site-loop and
fallback probes also match under `KILN_GC_STRESS=1`. Crate tests: `kiln-number-core` 12,
`kiln-native-provider-contract` 11 + 5, `kiln-builtins` 98, `kiln-backend-c` 187 + 36 (ABI
surface snapshot updated; the fixture prologue stubs `kiln_typed_array_sort`),
`kiln-interpreter` 197, `kiln-native-provider-ffi --all-features` 52.

**Left.** Callback methods other than `map`/`filter`/`reduce` (`forEach`, `some`, `every`,
`find*`, `reduceRight`), and every callback method inside a generic-only module, still run the
TypeScript at 80-150 ms per million: the lever there is the bootstrap's own compilation (hot
`%TypedArray%` methods specialized rather than generic `-O0`), not more site loops. `indexOf`,
`includes`, `lastIndexOf` and `reverse` remain element loops (~65-80 ms).
