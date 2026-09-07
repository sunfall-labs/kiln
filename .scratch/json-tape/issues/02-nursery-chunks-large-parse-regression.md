# Issue 02: 9d96e4a8 makes a 2.2 MB JSON.parse 50x slower on the native backend

Status: needs-triage
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
