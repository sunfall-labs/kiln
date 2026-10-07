# Bounded loop-callee O1 tier investigation

## Problem and scope

The unchanged `SchemaBinary` native run completed 192/193 original cases; the sole miss was `round-trips zero-width arrays across the allocation slack boundary`, which reached its original 5-second per-case deadline. The retained binary was profiled without changing that test or its deadline. This work changes only which existing generic boxed function bodies receive host-compiler O1; it does not change bytecode, call targets, calling convention, or runtime behavior.

Baseline input provenance: the pinned Effect source at `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace/source`, source revision `6e74e6ad` (the earlier CLI receipt records source `aee1648a`, docs-only later than production inputs), and frozen native CLI `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/.scratch/final-native-milestone/kiln-5c968ff32c38a8175754413088d5e62da0e39eefc2615750f1201124a99a806e`, SHA-256 `5c968ff32c38a8175754413088d5e62da0e39eefc2615750f1201124a99a806e`. Provider manifest SHA-256 is `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b` (ABI 82).

## Profile evidence

The stripped baseline executable was SHA-256 `652e11528dbba93b2878fe9ac5152e6ae2999ad7a32f87f28a0682ba48c1513a`. A symbolized cache-hit relink used the same emitted C and native objects; the relinked binary was `4b8cb18299ed74db86b416040ac2090bc0f252ebb7ad0e9c90d9d63aa5e7028e`, with identical `.text` start, size, and digest. The profile was diagnostic and overlapped other compile work; it is attribution evidence, not a quiet timing result.

Within the failing case window, the profile attributed 37.9% of samples to `decodeValueUnchecked` (proto 200, 873 IR instructions), 22.5% to `decodeValue` (proto 199, 83 instructions), 16.7% to `decodeArray` (proto 193, 594 instructions), and 8.5% to `encodeValue` (proto 165, 1,028 instructions). The decode call chain remains boxed and phase-local; the graph's known loop-call edges establish only ranking evidence. The current selector's full-module facts are reused for the split instantiation unit, while its per-unit availability and dispatch plans remain local.

## Change under review

The existing small source-order O1 bucket is unchanged. The supplemental generic tier now ranks bounded groups seeded by graph-proven known callees reached from loop call sites. It retains the generic boxed ABI and phase-local call behavior. Each group is limited to four bodies and the profile tier to eight additional bodies / 4,096 IR instructions; the older no-profile fallback remains at two bodies / 2,048 instructions. The existing function, instruction, graph-edge, and profile-construction bounds remain in force. No Effect or proto-name allowlist is used.

A source-derived fixture verifies that a loop-reached wrapper and its known leaf are selected together, that the loop caller itself need not spend the callee-group budget, and that unavailable callees remain excluded. The pinned SchemaBinary diagnostic exercises the actual instantiation emitter and confirms proto 199 and proto 200 receive O1 while phase-specific generic calls remain emitted. This establishes selector/emitter behavior only; the candidate release CLI and original 193-case replay have not yet been built/run, so no performance or acceptance claim is made here.

## Validation

Pinned Rust 1.97.1 / `mbx`, one worker, `--test-threads=1`:

- `cargo test -p kiln-backend-c --lib tests::direct_optimization -- --test-threads=1`: 7 passed, 0 failed, 2 ignored.
- `KILN_EFFECT_SOURCE_ROOT=<pinned Effect source> cargo test -p kiln-backend-c --lib diagnose_pinned_schema_binary_generic_optimization_selection -- --ignored --test-threads=1`: 1 passed.
- `cargo test -p kiln-backend-c --lib -- --test-threads=1`: 346 passed, 0 failed, 4 ignored.

The complete raw logs are retained under `evidence/`.
