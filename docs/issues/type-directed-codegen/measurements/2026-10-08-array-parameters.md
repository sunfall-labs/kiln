# Step 1, parameter half: checker facts for boolean and array parameters

Machine: 16 CPUs, Linux, shared; load average 16 to 60 during every measurement, so absolute times
are clock readings. Commit b14cc41d (plus this record) (work/typed-params). Binary built by `mbx build --release -p kiln-cli`.

## What changed

- `export_type`: plain `boolean` exports `Boolean` (it tested `BOOLEAN_LITERAL`, which the `boolean`
  union does not carry); array types (`T[]`, `ReadonlyArray<T>`) export `Array`, tuples `Tuple`.
  `CHECKER_VERSION` is `kiln-owned-v7`.
- `TypeAnnotation::Array`, hashed into the build identity as 8.
- `candidate_array_parameters_with_evidence` accepts the checker's array fact without a profile; the
  trace says `array_param[N]=annotation` or `=profile`. Same Array-kind entry guard, same
  `parameter_array` invariant row, same `KILN_SPECULATE` off switch and exit path.
- Fixed in passing: the entry guard declared `kiln_spec_<v>_arr` twice when a value was both a slot's
  initial value and a read of it (C redefinition error). The first probe build
  hit it on three functions, so it was reachable before only through a profile.

## array-param (new twin, bench/non-io/array-param.{ts,mjs})

Two kernels taking `values: number[]` (a sum of squares and a threshold count over 4096 elements).
Interleaved, 21 runs each, median of the harness's `warm_median`, checksums agree:

| build | median ms | min ms |
| --- | --- | --- |
| typed, facts used (`array_param:annotation=2`) | 3.688 | 3.500 |
| typed, `KILN_SOURCE_TYPES=0` | 5.875 | 5.688 |

1.59x. The untyped `.mjs` has no profile in this build, so it speculates nothing, like the withheld row.

## Twins that did not change

nbody, string-ops, array-functional, json-transform: speculation records are identical with this
commit (their arrays are module-scope upvalues or locals, not parameters; their strings come from
method chains the emitter already proves). Their timings in the script's table were taken at load
50 to 60 and are not recorded as measurements.

## Boolean and string parameters: not implemented, deliberately

- Boolean. A proven boolean's `b{v}` twin replaces `kiln_c99_val_truthy(v)` at a branch. That function
  is `static inline` and tests `FALSE`, `UNDEFINED`, `NULL`, then `TRUE` as immediate NaN-box
  comparisons before anything else (`runtime/prelude_types.h`), so a boolean test costs one to three
  compares on a register. The clone, the entry guard (`v == TRUE || v == FALSE`) and the `b{v}`
  materialisation cost as much as they save, and a boolean parameter is not on a hot arithmetic chain.
  `!x` and comparisons are already proven booleans without any parameter fact.
- String. `Proven::String` is read by nothing in the emitter. `is_proven_string` is a syntactic walk
  over `defs` that takes no assumptions, and what it removes is one `KILN_VAL_IS_STRING(v)` tag compare
  per intrinsic method call (`ops/call_method.rs`). Feeding it a parameter guess means threading
  speculation sets into `OpContext` (a change across `ops/`) to save a predictable compare, in
  functions that would also pay for a cloned body. The string twin has no string parameters.

Both facts stay in the annotation (`TypeAnnotation::Boolean/String`) for a use that does pay, such as
a string `length` or `charCodeAt` loop bound hoisted out of a loop; none exists today.

## `T | undefined` / `T | null`

Not exported. The backend has no representation for "double or undefined", so a class would be read by
nothing. Add it with the first consumer.

## Corpora (binary from this commit)

- `scripts/native.sh`: 665 matched, 5 refused, 0 diverged, 1 invalid probe (`fetch-live-response-cancel`:
  the interpreter cannot run it; network-dependent, matched in the next run).
- `KILN_SPECULATE=exit scripts/native.sh`: 666 matched, 5 refused, 0 diverged.
- Same five refusals both runs (kiln-extensions-off, native-callable-reflection, kiln-private-channel,
  regexp-surface-keys, shadow-realm).
- `scripts/thirdparty.sh`: 17 matched, 0 diverged (5 bundles not built on this machine); native 17 matched, 0 refused, 0 diverged.
- New probe `conformance/native/source-contract-array-parameter.ts`: output equals node's in default,
  `KILN_SPECULATE=exit` and `KILN_SOURCE_TYPES=0`.
