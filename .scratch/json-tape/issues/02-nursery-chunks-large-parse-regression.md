# Issue 02: 9d96e4a8 makes a 2.2 MB JSON.parse 50x slower on the native backend

Status: resolved
Feature: json-tape
Governing ADR: ADR 0041

## What

`origin/main` at ec54d4d0 parses a 2.2 MB JSON document in 325-350 ms (median) on the native
backend where ff22d23d takes 6.5 ms. Bisected with clean caches (`rm -rf .kiln/cache
target/kiln-test-cache`, then `mbx build --release -p kiln-cli` and `kiln build --profile
native-release` at each commit, in its own worktree):

| commit | large parse, warm median | warm best |
| --- | --- | --- |
| c7d71dc9 (parent) | 6.5 ms | 5.5 ms |
| 9d96e4a8 "The nursery is chunks, each reset or promoted on its own, inside a turn too" | 324 ms | 113 ms |
| ec54d4d0 (origin/main) | 326-348 ms | 123 ms |

The small payload (200 B) is unaffected (0.39 ms per 1000 stringify+parse on both sides).

## Profile

`scripts/profile-native.sh` on the ec54d4d0 binary, 961 samples:

```
   499 kiln_c99_gc_lookup_nursery      52%
    66 kiln_c99_gc_trace
    49 kiln_c99_gc_sweep
    48 kiln_c99_gc_lookup_slab
    46 kiln_c99_gc_mark_block
    25 kiln_c99_gc_alloc_ex
    23 _platform_memmove
    22 kiln_c99_json_read
```

The reader is 4% of the run. `kiln_c99_gc_lookup_nursery` was 6-7% before the change (profiles
in `.scratch/json-tape/NOTES.md`); a lookup that walks the nursery's chunks one by one would do
this once the nursery holds a few hundred thousand objects, which a 2.2 MB parse of 10,000
same-shaped rows does inside one turn.

## Repro

```
$ cp .scratch/json-tape/large.mjs .   # from json/rust-tape; 10k rows, 2.2 MB, bench/harness.mjs
$ kiln build large.mjs --profile native-release -o large && ./large
{"name":"json-parse-large", ... "warm_median":347.7, ...}    # ec54d4d0
{"name":"json-parse-large", ... "warm_median":6.5, ...}      # c7d71dc9
```

## Notes

- Not a cache artefact: every object in the bisect builds reports `(built)`, and the binaries'
  mtimes follow the checkouts.
- `json/rust-tape` is based on ff22d23d and does not carry the change; once rebased onto main it
  will inherit it, because the walk allocates the same strings, arrays and objects that the C
  reader does. Its own numbers in NOTES.md are against ff22d23d for that reason.

## Resolution (2026-09-06)

Each chunk is now one region aligned to its own size with the header and bitmap at its start,
and the nursery keeps a pointer set of region bases, the way the slabs do; a lookup is a mask and
a hash probe whatever the number of chunks. Measured on the 2.2 MB probe, native, this
worktree, against the unmodified compiler at ac301325:

| measure | before | after |
| --- | --- | --- |
| cold first parse (harness) | 111-113 ms | 13.9-14.2 ms |
| per parse, fixed loop of 20 vs 40 in one turn (slope) | 360-375 ms | 20.0-20.9 ms |
| profile share of the chunk lookup | 52% | 16% (batch 8, heavier garbage; 6-7% before 9d96e4a8 at batch 1) |
| small payload, warm median | 0.404 us | 0.409 us |

Checksums equal on both probes under `KILN_GC_STRESS=1` (small) and `=200` (large).

## Three-way measurement after the fix (2026-09-06, later)

Fixed-count slope (20 vs 40 parses of the 2.2 MB probe inside one turn, so the harness's batch
choice cannot differ between binaries), five interleaved rounds, best / p25 / median, plus the
harness's cold first parse and the small payload. Load average 6 throughout; checksums equal.

| build | slope ms/parse | cold ms | small us | collections per 20 parses |
| --- | --- | --- | --- | --- |
| parent c7d71dc9 (single-region nursery, C reader) | 5.74 / 6.55 / 6.62 | 5.52 / 5.63 / 5.64 | 0.40 | 8-10 |
| origin/main with 9d96e4a8 reverted (single-region nursery, Rust reader, everything else) | 6.29 / 6.57 / 6.76 | 6.08 / 6.09 / 6.15 | 0.40 | 9 |
| origin/main as landed (chunks, masked lookup d7428e9a) | 18.33 / 20.53 / 22.03 | 13.36 / 14.41 / 14.70 | 0.40 | 81 |
| origin/main + adaptive chunk budget (branch `fix/nursery-lookup-2`, not landed) | 6.94 / 7.20 / 7.43 | 6.06 / 6.11 / 6.24 | 0.40 | 13 |

The revert row is at parity with the parent, so the reader and every other commit since
c7d71dc9 cost nothing per parse; the whole residual is 9d96e4a8's nursery. The masked lookup
removed the walk (52% of the profile, first parse 111 ms) but main as landed is still 3x the
parent per parse, and the collector's own trace says why: the chunk-budget trigger makes a
collection due every 4 MB of filled chunks whatever the last pass found, so a parse whose result
is all live is collected four times as it grows and marks its own result each time -- 81
collections for twenty parses against 8. An earlier two-way that reported 21 ms for main was
taken while another agent ran an all-core benchmark; this run repeats it at the same load as
the parent and the number stands.

The adaptive budget (a pass whose chunks mostly promoted doubles the wait for the next one, up
to eight budgets; a pass that mostly reset brings it straight back) takes main to 13
collections and within 10-13% of the parent on the slope and 1% of the revert on cold. What
remains after it is the design: a hash probe per traced pointer (`kiln_c99_gc_lookup_nursery`
16% of the profile against 6-7% before 9d96e4a8, when the nursery was one range check) and the
per-chunk settle walk. Closing that needs the chunks in one reserved address range so the
lookup is a range check again, which is a design decision, not a fix.
