# Extended collection and TypedArray investigation

Base: `80f55d78`. Pinned Test262: `771005236e88a909635104e03ba12559688c0172`.
All ordinary probes retained the 8,000,000-step execution limit, four-worker CPU limit,
and 4096 MiB memory cap. No budget, skip, baseline or extended-classification changes.
Each executable was copied into this worktree before measurement. Baseline SHA-256:
`77a6c7823e45604ee76df3073e96b4cc63da43e7f292c412bd17003000db58e2`.
Changed production executable SHA-256:
`c085ca0be5ca66c9470cf5c5524cd65e2b7f1735b97758e6a2811af0c0773861`.

## Changes and results

Collection construction previously ignored the public `add`/`set` override and invoked
native insertion directly. An infinite iterator therefore exhausted the budget instead
of stopping when the overridden adder threw. The constructor now gets and validates the
adder before opening the iterator, captures it once, and calls it for each entry. The
existing close-on-abrupt path preserves the original throw even if `return` throws.
Weak collections use the same path; overridden weak adders can accept primitive keys.

The three Map/Set files (six variants) now pass in **0.44–0.92 ms per variant**, versus
**518–1503 ms** before, when all exhausted the step budget. Exact individual timings and
outcomes are in `measurements.json`.

Numeric TypedArray reads/writes now retain their numeric key instead of formatting a
property name, allocating UTF-16 storage and parsing the index back. The guard requires
a real TypedArray and numeric primitive key. Numeric `-0` becomes index zero; the string
`"-0"`, object-key coercion, proxies and ordinary names retain their existing paths.
Writes share the existing coercion-before-bounds semantics with canonical string keys,
including detach/resize during conversion. Instruction charging is unchanged.

A standalone 100,000-byte JavaScript copy probe, run alternately three times under the
same 8M limit, took **183.483, 183.674, 182.820 ms** before and
**142.896, 142.677, 142.602 ms** after (external process time, including startup).
This is a representation improvement, not a claim of completed conformance.

## TypedArray budget diagnosis

The three full-size `copyWithin/coerced-values-{end-detached-prototype,end-detached,start-detached}`
files still fail at exactly 8M steps. Their elapsed times are effectively unchanged in
these concurrent-case measurements: 5.73–6.48 seconds before, 5.80–6.53 seconds after.
Do not describe these as nontermination or as resolved failures.

These tests use 10,000-element arrays and repeat their assertion across every numeric
TypedArray constructor and the harness's constructor argument factories. Diagnostic
copies that throw a marker before each factory show execution reaches and completes
passthrough, array, arraylike, iterable, fixed-buffer and resizable-buffer factories,
then runs out of fuel during the grown-buffer factory, before the shrunk-buffer factory.
`copyIntoArrayBuffer` copies every byte with JavaScript indexed operations for each type;
`makeArrayLike` also creates and reads 10,000 ordinary object properties. This is finite
setup work. Faster property representation can lower its wall cost without changing
its instruction requirement.

In explicitly separate diagnostic copies, changing only array length from 10000 to 1000
preserves the assertions and all tested source/end indices. All six variants pass both
before and after under the ordinary 8M budget, across every original factory. These are
**diagnostic copies**, not results for the pinned corpus. Their whole batch fell from
326 ms to 284 ms. The pinned checkout was never modified.

At base `80f55d78`, an additional pre-existing ownership issue was observed separately: normal `kiln run`
uses the shared `typedarray.ts` `copyWithin`, whose copy loop lacks the post-coercion
buffer-detachment check. A small standalone module therefore misses the expected throw.
The Test262 path's existing implementation passes the reduced diagnostic cases. That
separate semantic/ownership issue is not the cause of the target budget exhaustion and
is not changed by this patch.

## Validation

Focused source regressions cover all four collection constructors, adder getter order,
original-vs-close error precedence, captured adder mutation, argument count and receiver,
non-callable rejection before iterator opening, and null iterable handling. Numeric-key
regressions exercise both strictness modes, numeric/string boundaries, inherited numeric
getters, key and value coercion, proxy traps, BigInt refusal at an invalid index, and
buffer detach/grow/shrink during value conversion. Raw fixture detachment uses the
existing storage primitive when raw execution has no TypeScript bootstrap, otherwise the
public transfer method.

`mbx test -p kiln-conformance --lib`: **71 passed, 0 failed**.

Pinned subtree comparisons verify identical case IDs and no passing-to-nonpassing changes:

| Subtree | Cases | Before pass | After pass |
| --- | ---: | ---: | ---: |
| Map | 405 | 391 | 401 |
| Set | 764 | 746 | 754 |
| WeakMap | 281 | 271 | 281 |
| WeakSet | 170 | 160 | 170 |
| TypedArrayConstructors/internals | 454 | 326 | 326 |

Thus the constructor correction gains **38 passing variants**, including the six targeted
extended variants. The three TypedArray files remain budget failures, explicitly recorded.

Reproduction uses `scripts/test262_corpus.run_result(kiln, root, out, log, cases=selection)`
with `KILN_JOBS=4 KILN_TEST262_MEMORY_MB=4096 PYTHONPATH=scripts`. Selection JSON contains
file IDs relative to the chosen subtree; the runner executes both original variants and
retains their harness/fixtures. Subtree comparisons omit `cases`. Raw logs and JSON are
retained locally in `docs/issues/extended-limits/evidence/`; they are not committed.
