# Routine and extended Test262 coverage

`mise run test262` runs the routine section. `mise run test262-extended` runs only the expensive
section. `mise run test262-full` runs both. The corresponding gate commands are `mise run gate`
and `mise run gate-full`. Existing `scripts/test262.sh --out report.json --path <subtree>` still
runs every case in that pinned subtree, regardless of its classification here.

The split now defers **499 files / 998 variants**, leaving **101,640 routine variants**.
The original 502 files accounted for 88% of summed case durations in the recorded full run.
Three Map/Set iterator-close files were promoted after fixing constructor adder dispatch:
all six variants now pass in under a millisecond instead of consuming the instruction budget. Classification
is explicit in `test262-extended.json`, not recalculated from noisy timing on every invocation.
All variants of each selected file execute. Reports declare which suite ran and how much was
deferred; a routine report is never evidence that the complete corpus passed.

## What “extended” means

This is an execution-cost policy, not a claim that these tests inherently need seconds:

| Files | Workload | Interpretation |
| ---: | --- | --- |
| 448 | Unicode/RegExp sweeps | Large inputs; guarded indexed writes, numeric dispatch and string storage improvements reduced measured wall time by 25% |
| 3 | TypedArray copyWithin cases | Finite byte-by-byte harness setup exhausts the eight-million-step limit; all assertions complete in a separately labeled reduced-size diagnostic |
| 48 | Mixed compiler, property, URI, iterator and calendar workloads | Constant interning and key membership had quadratic costs; profiling separates finite budget exhaustion from an erroneous iterator-close loop |

A representative Unicode-builder probe produced identical output in Node and Kiln. Median
end-to-end process times over three runs were 56 ms and 1.11 s, respectively. These include
process startup and are triage measurements, not steady-state benchmark claims or measurements
of every Unicode case. Map/Set iterator-close tests expect an overridden insertion method to
throw promptly; consuming an infinite iterator until the step limit was a correctness problem, now fixed.

The optimization evidence lives under `.scratch/extended-unicode`, `.scratch/extended-limits`,
and `.scratch/extended-mixed`. Those controlled comparisons name their pre-bootstrap baseline
`80f55d78`; combined validation must also include the newer common bootstrap in every realm.
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
