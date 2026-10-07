# Nursery lookup memo validation (2026-10-06)

This candidate reuses the first nursery-region lookup for a string's header-adjusted
candidate only when both addresses remain in the same region. If adjustment crosses a
region boundary, lookup falls back to `nursery_locate`. Region membership uses integer
addresses, avoiding relational comparisons between unrelated C pointers.

The source is based on `ab63eaee` plus the candidate commits `836078a9` and `53007d6b`;
final source tree: `85f545122442247a1a215d3cc960285f92030ede`.

Focused backend validation on pinned Rust 1.97.1 passed:

- `executable_gc_lookup_string_header_falls_back_at_region_start`: 1 passed.
- `executable_minor_gc_traces_young_values_published_by_tenured_cells`: 1 passed.
- The preceding `tests::objects::` run passed 86/86 before the final pointer-comparison
  cleanup; the two focused tests above were rerun on final source.
- Native parity for `owned-string-boundary-gc.js` and `async-nursery-pressure.js`:
  2 matched, 0 refused, 0 diverged. With `KILN_NATIVE_GC_STRESS=1`: 2 matched,
  0 refused, 0 diverged, 0 stress probes skipped.

The original pinned Effect Hash native-release replay remains 24/25, with no skips and
all 25 cases mapped to Node/Vitest. The only failure is the existing 5000 ms timeout in
`Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place
edits`; it is a timeout, not an output mismatch. This lookup memo alone did not clear
that deadline.

Build provenance:

- CLI source: final tree above; CLI SHA-256
  `6217c7b018d88341b986749d54b0b37fc08af0212a2805a41d2d939dbc53dad6`.
- ABI-82 provider manifest SHA-256
  `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
- Hash entry SHA-256 `70701e3cb9994e7d8522a5e1c84123d6ceeb33b751b3de3820fde8aaa84c1cbc`.
- Native binary SHA-256
  `bc820a90860cd53fee311715ea5fa9405863734bcd7ddc31413506b3b1292b0e`.
- Native build: 710.536 s total; emission 21.648 s, compile 686.915 s, link 0.299 s.
  Generated C 184,907,386 bytes; app object 166,994,848 bytes.
- Runtime acceptance used the original per-case 5000 ms deadlines. The quiet replay
  recorded 24 passes and the one timeout above.

The compressed test and runner receipts are in [`evidence.tar.zst`](evidence.tar.zst)
(SHA-256 `10bb11c9862e0220f62a2c5cd19b543770d88f44974d530dfc5403332be0a21e`). The
candidate CLI and native executable remain in the owned worktree's ignored
`.scratch/nursery-memo-acceptance/` directory and are intentionally not committed.
