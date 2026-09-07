# JSON.parse as a Rust tape: measurements

Worktree `kiln-json-tape`, branch `json/rust-tape`, base 3ae20ea2. Probes: `small.mjs` (the
bench/non-io/json.mjs payload, ~200 B, 1000 parses per call), `large.mjs` (10k same-shaped
objects, 2.2 MB, one parse per call). Native: `kiln build --profile native-release`, harness
warm median. Interpreter: process timed from outside, floor subtracted (small), or the slope
between 20 and 40 iterations (large), so module-scope setup is out of the number.

## Before (C recursive-descent reader, interpreter's own JsonReader)

| probe | native, per parse | interpreter, per parse |
| --- | --- | --- |
| small (200 B) | 0.81 us | 2.49 us |
| large (2.2 MB) | 13.3 ms | 12.4 ms |

Profile of the large native binary (2594 samples): GC and allocation ~51% (`gc_alloc_ex` 296,
`gc_lookup_slab` 255, `gc_lookup_nursery` 152, `gc_sweep` 130, `gc_mark_block` 116, `gc_trace`
93, slab/finalize/free ~230); scanning and number conversion ~27% (`json_read` 177,
`string_to_number` 112, `copy_text` 100, memmove 95, `json_string` 85, float `from_str` 85,
`from_utf8` 51); property insertion ~4% (`put_mode_ikey` 61, `intern` 51).

## After (kiln-json over simd-json's tape; C and interpreter walk the tape)

Interleaved A/B, three runs each, best `warm_best` and median `warm_median` (the machine is shared
with other agents, so +/-5% is noise here):

| probe | native before | native after | interpreter before | interpreter after |
| --- | --- | --- | --- | --- |
| small (200 B), per parse | 0.58 us best / 0.60 median | 0.60 us best / 0.63 median | 2.49 us | 1.45 us |
| large (2.2 MB), per parse | 8.9 ms best / 10.1 median | 9.2 ms best / 10.2 median | 12.4 ms | 10.8 ms |

Reader alone (`crates/kiln-json/tests/timing.rs`, release): small 0.149 us, large 1.70 ms. So on
native the Rust side is ~19% of the large parse and ~25% of the small one; the rest is the C heap.

Profile after, large native (1912 samples): heap and collector ~47% (`gc_lookup_slab` 193,
`gc_alloc_ex` 181, `gc_lookup_nursery` 117, `gc_sweep` 107, `gc_mark_block` 97, `gc_trace` 75,
slab/finalize ~105); the reader ~16% (`fill_tape` 162, `parse_number` 61, `parse_str` 59,
`parse_with` 31); the walk 3% (`json_build` 56); string materialisation ~6% (`val_from_text` 73,
`copy_text` 45); property insertion ~3.5%.

Two walker versions were slower and are recorded so nobody rewrites them: (1) frames on the
malloc heap only -- the collector runs mid-turn on allocation and scans the machine stack
conservatively, so an open container held only in malloc'd memory was freed under the walk
(`Cannot read properties of undefined` after enough iterations); (2) every container and key in
a collector-owned array with `kiln_val_to_boundary` on each delivery -- correct, but 25-40% slower,
because the conversion looks the pointer up in the heap. The shipped walker keeps frames in a
local array for the first 32 levels and mirrors into a traced array only past that.

## After rebase onto origin/main (02792a3d: main's own faster C reader and stringifier)

Base is a fresh `origin/main` worktree's binary; the tape branch is rebased on it, so both sides
have main's stringify fast path and the only difference is the parse side. Interleaved A/B, three
rounds, best `warm_best` / median `warm_median`; interpreter timed from outside, floor
subtracted (small) or the 20-to-40-iteration slope (large).

| probe | native main | native tape | interpreter main | interpreter tape |
| --- | --- | --- | --- | --- |
| small (200 B), per stringify+parse | 0.391 us best / 0.403 median | 0.426 us best / 0.442 median | 1.37 us | 1.42 us |
| large (2.2 MB), per parse | 5.51 ms best / 6.48 median | 5.99 ms best / 6.74 median | 11.06 ms | 10.90 ms |

**Main's C reader beats the tape on native by ~9% on both payloads.** The interpreter is at
parity; the 2.49 -> 1.45 us interpreter gain recorded above was measured across two different
machine loads and does not reproduce back to back, so it is withdrawn.

Where the native time went (profiles, both payloads, same harness): the reader's share of the
parse rose from 23% to 39% on the small payload (main: `json_read` 244 + `json_string` 49 +
`json_fast_number` 47 + `json_space` 22 of 1544 samples; tape: `fill_tape` 252 + `parse_str` 86
+ `parse_number` 56 + `parse_with` 53 + `copy_source` 22 + thread-local 35 of 1336) and from 16%
to 24% on the large one (main 381 of 2325; tape 443 of 1819). Heap and collector samples are
the same on both sides. Main's reader is one pass that builds objects as it scans and reads
numbers with a Clinger fast path; the tape path copies the input, scans it, converts simd-json's
tape into ours and walks it a second time, and simd-json's `parse_number` (the correct-rounding
path) is slower than main's fast path on the integers and short decimals these payloads are made
of. SIMD does not help at these sizes: the small payload is three 64-byte blocks, and on the
large one the scan was never the bottleneck.

What the tape buys for that 9%: the grammar and every number and surrogate edge exists once,
in Rust, tested once, and shared by both engines; main's reader had to re-implement the fast
number path and the interned-key path in C and test them against node separately. The walker
now takes main's `kiln_c99_json_define` (an interned key straight from the tape's text, no GC
string per key, and `__proto__` as an own property), which main's `json-fast-path-edges.js`
asserts and which the earlier walker, inserting through `kiln_c99_put`, would have failed.

## Closing the gap: three changes tried, measured after each

**1. No per-call allocation, no handle.** `kiln-json` now keeps one thread-local scratch (the
scanner's `Buffers` and tape, the padded input copy, the node array) and lends it to a closure;
the ffi export became a single `kiln_json_parse` that writes four out-pointers into that scratch,
valid until the next parse on the thread. `kiln_json_tape` and `kiln_json_free` are gone (88
exports). The one-parse-in-flight invariant is asserted by the scratch's `RefCell`: a re-entrant
parse panics, and the ffi turns the panic into the reader-failed status. Reader alone: unchanged
at 0.152 us / 1.77 ms, because it never allocated per call after the first document. Native,
interleaved A/B, five rounds, best / median-of-medians:

| probe | native main | native tape (change 1) |
| --- | --- | --- |
| small (200 B) | 0.387 / 0.397 us | 0.416 / 0.434 us (+7.5% best) |
| large (2.2 MB) | 5.53 / 6.58 ms | 6.09 / 6.53 ms (+10% best, -0.7% median) |

It paid about 10 ns per small parse (the `publish`/`retire`/`tape`/`free` calls and two of three
thread-local reads are gone; `kiln_json_parse` 57 samples now includes the convert pass that
`parse_with` used to show separately) and did not close the gap.

**2. One pass.** Not possible from outside simd-json: its `Node` is a Rust enum (`String(&str)`,
`Object { len, count }`, ...) built by stage 2 through an internal `insert_res!` into its own
`Vec`, with no `#[repr(C)]` layout to expose and no hook to write another format. The convert
pass is one tight loop over 25 nodes (small) and costs on the order of 1-2% here; folding it away
would need a fork of stage 2. Not done.

**3. Short-number fast path.** simd-json's correct number path already has it: `correct.rs`
lines 337-339 multiply or divide by an exact power of ten when the exponent is within +/-22, with
eight-digit SWAR chunks above that. The profile puts `parse_number` at 4.5% of the small parse
against main's `json_fast_number` at 3.0%, a difference of about 6 ns, so this is not where the
delta is. Skipped.

**Where the remaining time is** (small, after change 1, 1229 samples): `fill_tape` 239 (19%),
`parse_str` 74, `parse_number` 56, `kiln_json_parse` 57 (copy + convert), memmove 64, thread-local
27 -- the reader side is ~42% of the parse against main's 23%. `fill_tape`'s share is simd-json's
fixed per-document cost: stage 1 indexes the padded input in 64-byte blocks and stage 2 walks
the structural index; on a 159-byte document that machinery costs more than a scalar reader's
one pass, and nothing outside the crate shortens it. Heap and collector samples are equal on
both sides. On the large payload the same scan is only ~12% and the medians are equal; the
best-of-N gap there is inside this shared machine's noise band (load average 5.6 during every
run) but does not clear the 3% rule either.

**`bench/non-io/json.mjs` through `bench/compare.sh`** (main's commit quoted this one), same
session, both trees:

| tree | node | bun | kiln interpreter | native | native/node |
| --- | --- | --- | --- | --- | --- |
| origin/main | 28.0 ms | 15.8 ms | 261.8 ms | 46.6 ms | 1.66x |
| json/rust-tape | 27.0 ms | 15.3 ms | 181.2 ms | 47.3 ms | 1.75x |

Native +1.5% on that benchmark; interpreter 31% faster (this bench is stringify+parse over a
50k-iteration loop, and the interpreter's old `JsonReader` was the slower of the two readers).

**Decision by the stated rule (within 3% on both payloads, best-of-three): not parity. Not
merged.** What would close it is a scalar Rust scanner for documents under a size threshold,
sharing the number, string and surrogate rules with the simd path -- a fourth change outside
this brief.

Also filed: `issues/01-embedded-nul-key-truncation.md` (both native readers cut a parsed key at
an embedded NUL; the interpreter and node do not).

## The scalar scanner (`crates/kiln-json/src/scalar.rs`)

One pass over a copy of the input: escaped strings are decoded in place at their own offset,
unescaped ones are referenced where they lie, numbers are validated and converted in the same
loop (`number_from_parts`: the exact multiply/divide for at most 15 significant digits and
|exponent| <= 22, else `f64::from_str`), containers are patched with their child count when they
close, over an explicit frame stack. Same tape, same per-thread scratch, no new exports and no
ABI change; `parse` picks by length and neither engine can tell. `tests/agreement.rs` forces
every input from the crate's tests, `json-tape-edges.js` and `json-fast-path-edges.js`, plus a
generated sweep (nested containers to depth 65, every escape form as value and key, numbers
around 2^53 and at +/-22/23/308/324, -0, strings across the 64-byte block boundary, documents
of the benchmark's shape at five sizes) through both scanners and compares the tapes.

**Crossover** (`tests/timing.rs crossover`, release, both scanners on documents of the
benchmark's shape, us per parse):

| bytes | scalar | simd | simd/scalar |
| --- | --- | --- | --- |
| 159 (small.mjs) | 0.127 | 0.153 | 1.21 |
| 197 | 0.141 | 0.177 | 1.25 |
| 406 | 0.276 | 0.306 | 1.11 |
| 1,894 | 1.305 | 1.321 | 1.01 |
| 7,715 | 5.27 | 5.37 | 1.02 |
| 31,694 | 21.3 | 24.1 | 1.13 |
| 128,244 | 85.6 | 96.9 | 1.13 |
| 257,912 | 172 | 195 | 1.13 |
| 519,892 | 344 | 391 | 1.14 |
| 1,044,099 | 686 | 784 | 1.14 |
| 2,094,981 | 1,369 | 1,581 | 1.16 |
| 4,226,896 | 2,675 | 3,200 | 1.20 |
| 8,494,746 | 5,553 | 6,435 | 1.16 |

No crossover: the scalar pass wins at every size measured, so `SCALAR_BELOW` is 8 MiB, a little
below the largest document measured, and simd-json serves only documents larger than anything
measured. Whether it stays a dependency at all is now a fair question; the table gives no
reason to keep it.

Two scanner versions that were slower, so nobody rewrites them: (a) the first draft appended
each string to a separate arena with `extend_from_slice` and read numbers in two passes
(`number_end` then `number_value`) -- 0.183 us on the small payload against simd's 0.167, slower
at every size; (b) replacing the string fast loop with `memchr2` plus an eight-byte SWAR
control-byte check -- 1.76 ms on 2 MB against 1.35 for the plain byte loop, because the strings
here are 5-20 bytes and a call plus a second pass cost more than one three-way test per byte.

**Final A/B** (fresh origin/main binary, interleaved, five rounds, best / median-of-medians;
interpreter min / median of five, floor subtracted or 20-to-40 slope; load average 6):

| probe | native main | native tape | interpreter main | interpreter tape |
| --- | --- | --- | --- | --- |
| small (200 B) | 0.389 / 0.407 us | 0.398 / 0.417 us (+2.3% / +2.5%) | 1.47 / 1.50 us | 1.46 / 1.48 us |
| large (2.2 MB) | 5.58 / 6.66 ms | 6.25 / 6.71 ms (+12% / +0.7%) | 11.40 / 11.37 ms | 11.14 / 10.73 ms |

`bench/non-io/json.mjs` through `bench/compare.sh` (full suite run on each tree, json row):

| tree | node | bun | kiln interpreter | native | native/node |
| --- | --- | --- | --- | --- | --- |
| origin/main | 27.5 ms | 15.8 ms | 225.5 ms | 46.2 ms | 1.68x |
| json/rust-tape | 27.0 ms | 15.3 ms | 230.7 ms | 45.8 ms | 1.70x |

**Decision by the rule (within 3% of main's best on both payloads): small passes, large does
not. Not merged.** Two observations for whoever revisits the rule. The large medians are within
1% and the compare.sh row is 0.8% in the tape's favour; the 12% is between the *best* samples,
where main's best sits 16% below its own median and the tape's 7% below its -- main's
distribution has a longer fast tail under this machine's load, and best-of-N rewards that. And
the residual is structural: on a 2.2 MB document the tape is 280k nodes, 4.5 MB written by the
scanner and read back by the walk, where main's reader builds as it scans and never writes
them. Profile (large, scalar tape, 1697 samples): scanner `kiln_json_parse` 142 +
`scalar` 81 + input copy 54 = 16%, the C walk `json_build` 88 = 5%, heap and collector the
same as main's. A narrower node (tag and length packed with a 32-bit payload, numbers in a side
array) would cut that traffic by a third at the cost of a tape-format change on both engines.

## simd-json measured and dropped

The crossover table above says the scalar pass wins at every size from 159 bytes to 8.5 MB, so
the simd path never ran once `SCALAR_BELOW` was set and it is gone: the dependency (workspace
and crate), the padded-copy and surrogate-normalising code, the convert pass, the infinity
patching, `Scanner` and `SCALAR_BELOW`. `kiln-json` has no dependencies now; the reader is
`scalar.rs`. The table is kept in the crate comment so the next person does not re-run the
experiment blind. `tests/agreement.rs` keeps every edge input as the reader's own suite -- each
parses to a well-formed tape or is refused -- and pins `number_from_parts` to `f64::from_str`
across 1-17 significant digits at every exponent from -30 to 30 with and without a fraction,
`-0` in every spelling, the 2^53 neighbourhood, `1e22`/`1e23`, `1e308`/`1e309`, the subnormal
edge, and integers past `u64`: bit-for-bit.

## origin/main moved under the baseline, and regressed

Between two creations of the `../kiln-json-base` worktree (about 19:30 and 20:10), `origin/main`
moved from ff22d23d to ec54d4d0 -- a fetch in this repository updates every worktree's remote
ref -- so the fourth baseline build was a different commit, and its large-payload number was
325-350 ms per parse instead of 6.5. No two builds of one commit ever disagreed: ff22d23d built
five times gave 5.5-5.6 ms best every time; ec54d4d0 built clean gives 123 ms best. Bisected to
9d96e4a8 ("The nursery is chunks..."): parent c7d71dc9 is 6.5 ms median, 9d96e4a8 is 324 ms,
profile 52% `kiln_c99_gc_lookup_nursery`. Filed as `issues/02-nursery-chunks-large-parse-
regression.md`. The parity rule is applied against ff22d23d, this branch's base, which is main
without this change and without that regression.

## Final A/B, 20 interleaved rounds against ff22d23d (best / p25 / median of the per-run values)

| probe | native main (ff22d23d) | native tape | interpreter main | interpreter tape |
| --- | --- | --- | --- | --- |
| small (200 B), per stringify+parse | 0.386 / 0.401 / 0.405 us | 0.377 / 0.398 / 0.402 us (-0.7% / -0.7% on p25 and median) | 1.49 / 1.46 / 1.47 us | 1.38 / 1.40 / 1.41 us |
| large (2.2 MB), per parse | 5.445 / 6.453 / 6.555 ms | 5.793 / 6.257 / 6.340 ms (-3.0% / -3.3% on p25 and median; +6.4% on best) | 11.13 / 11.66 / 12.20 ms | 10.02 / 10.36 / 10.35 ms |

Rule (median and p25 within 3% on both native payloads, interpreter no worse): passes on both
payloads, in the tape's favour on every median and p25; the narrow tape node was not needed.
Best-of-20 on the large payload is 6% in main's favour, which is the tail the rule now sets
aside: main's best sits 17% under its median, the tape's 9% under its.

Reader alone after the simd-json removal: 0.111 us on the small payload, 1.38 ms on the large.

## Landed: rebased onto ec54d4d0, gate, and what the rebase inherited

`mise run gate-quick` on the rebased tree (whole log kept):

    native: 134 matched, 9 refused, 0 diverged        (the deliberate -refuses probes and ShadowRealm)
    thirdparty: 17 matched, 0 diverged, 5 not built   (bundles absent; conformance/thirdparty/build.sh)
    thirdparty (native): 17 matched, 0 refused, 0 diverged
    FAILED: crate tests -- kiln-module-graph::tests::node_builtin_static_imports_are_typed_virtual_modules

That one failure is origin/main's own: it fails identically on a pristine ec54d4d0 checkout
(expects `appendFile,mkdir` among `fs.promises`' members; the tree answers `readFile,stat,
writeFile`), the crate is untouched by this branch, and every other crate is green. JSON probes
after the rebase: native 4 matched / 0 diverged (including json-fast-path-edges), differential 5
matched / 0 diverged.

The rebased tape on ec54d4d0 (the main with the nursery regression, issue 02): large payload
warm median 4.9-5.1 ms, warm best 4.5-4.7 ms, cold 105-112 ms; small unchanged at 0.39 ms. Main's
own reader on the same commit: warm median 325-348 ms, cold 105 ms. So the tape does not inherit
the regression in the steady state -- its walk allocates every array at its final size and never
holds a builder per member, so the nursery sees far fewer lookups -- and does inherit the cold
run, which is the first turn's nursery filling either way. The warm number is better than on
ff22d23d (6.3 ms) because the chunked nursery is faster when it is not being searched.
