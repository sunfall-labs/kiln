# Routine and extended Test262 coverage

`mise run test262` runs the routine section. `mise run test262-extended` runs only the expensive
section. `mise run test262-full` runs both. The corresponding gate commands are `mise run gate`
and `mise run gate-full`. Existing `scripts/test262.sh --out report.json --path <subtree>` still
runs every case in that pinned subtree, regardless of its classification here.

The split now defers **58 files / 116 variants**, leaving **102,522 routine variants**.
The original 502 files accounted for 88% of summed case durations in the recorded full run.
Three Map/Set iterator-close files were promoted after fixing constructor adder dispatch:
all six variants now pass in milliseconds instead of consuming the instruction budget. Classification
is explicit in `test262-extended.json`, not recalculated from noisy timing on every invocation.
All variants of each selected file execute. Reports declare which suite ran and how much was
deferred; a routine report is never evidence that the complete corpus passed.

## What “extended” means

This is an execution-cost policy, not a claim that these tests inherently need seconds:

| Files | Workload | Interpretation |
| ---: | --- | --- |
| 7 | Remaining RegExp sweeps | Other large regular-expression inputs remain in extended coverage; all 441 generated Unicode property-escape files moved to routine after the complete run measured a 650 ms median, 771 ms p90 and 943 ms maximum per variant, with all 882 variants passing |
| 3 | TypedArray copyWithin cases | Earlier implementation exhausted its budget on finite setup; latest main's shared implementation instead fails promptly on missing post-coercion detachment checks, still unresolved |
| 48 | Mixed compiler, property, URI, iterator and calendar workloads | Constant interning and key membership had quadratic costs; profiling separates finite budget exhaustion from an erroneous iterator-close loop |

The 441 promoted property-escape files account for 882 routine variants. Together they summed
583.2 seconds of per-case execution time in that full run, but each individual case completed in
under one second. The full baseline and inventory digest are unchanged; the manifest's extended
reference count for `built-ins/RegExp` drops from 896 to 14 passing variants.

A representative Unicode-builder probe produced identical output in Node and Kiln. Median
end-to-end process times over three runs were 56 ms and 1.11 s, respectively. These include
process startup and are triage measurements, not steady-state benchmark claims or measurements
of every Unicode case. Map/Set iterator-close tests expect an overridden insertion method to
throw promptly; consuming an infinite iterator until the step limit was a correctness problem, now fixed.

The optimization evidence lives under `docs/issues/extended-unicode`, `docs/issues/extended-limits`,
and `docs/issues/extended-mixed`. Those controlled comparisons name their pre-bootstrap baseline
`80f55d78`. The [combined report](../docs/issues/slow-case-opt/README.md) validates the newer common
bootstrap in every realm: all 102,638 variants ran, with no chunk regressions and 43 gained passes.
They do not claim that time to a fuel limit is the time needed to finish an exhaustive test.

Run the extended section periodically, before releases, and when changing features it exercises.
Keep optimizing its entries and promote files back into routine coverage after a complete run
supports the change. Use the subtree runner for focused work; do not lower execution budgets to
make a long failure appear faster.

## Coverage and baseline contract

The manifest pins every chunk's file count, variant count and SHA-256 of sorted variant IDs for
Test262 commit `771005236e88a909635104e03ba12559688c0172`. Discovery must reproduce that inventory.
The scheduler verifies that every batch returns exactly its expected IDs and that the union
covers the selected section once. Missing or duplicate results, crashes and cap violations fail.
The original corpus paths, harness and module fixtures remain in place; no partial copied corpus
or result cache is involved. CPU and memory limits are divided between the batch processes.

The full `test262-baseline.json` has not been lowered. Extended reference passing counts are
subtracted from each full chunk's passing floor to obtain the routine floor. Full runs still
check the original floor. Existing full-baseline regressions therefore remain visible.
`mise run test262-update` requires full coverage and deliberately updates both baseline documents;
changing the classification or corpus pin also requires reviewing the inventory and split.

Each run retains `plan.json`, per-chunk inventories and merged results, batch ID lists, raw logs,
and batch results under `target/test262-baseline-logs/run.<id>/`. Timing estimates affect only
batch order. They never determine whether a test runs.
