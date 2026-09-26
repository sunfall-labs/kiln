# JSON numeric parser profile

Temporary isolated integration test `crates/semantics/kiln-json/tests/numeric_profile.rs` ran the same 200-number JSON array for five seconds in optimized release mode. The baseline was `/Users/alee/Developer/Personal/kiln-json-baseline`; the candidate was this worktree. `sample` ran for ten five-second samples after an 0.8-second warmup.

- Baseline: 3,954,000 parses; the sampled hot stack is the monomorphized scalar scanner and has no separately sampled number conversion function.
- Candidate before the fix: 3,419,000 parses; `sample` and disassembly show an out-of-line `number_from_parts` call for every token (`bl __...number_from_parts` at `0x1000019c0`).
- Candidate after `#[inline(always)] number_from_parts`: 3,849,000 parses; the conversion symbol disappears from the sampled stack and the scanner accounts for 3,409/3,412 samples. The remaining gap is about 2.7% on this fixture.

The temporary test is removed after this diagnosis. The retained matched harness accepts `KILN_JSON_BENCH_FILTER=number-heavy` and `KILN_JSON_BENCH_MIN_MS=5000` for a reproducible longer numeric run. The inline annotation is retained with a comment at its call boundary. The source fixture and exact mbx command are in the task history; this directory preserves the sampled evidence and candidate assembly.
