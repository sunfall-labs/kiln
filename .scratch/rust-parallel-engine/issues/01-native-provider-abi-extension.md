# Issue 01: Native Provider ABI Extension & Export Verification

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0018, ADR 0025, ADR 0058

Reconciled 2026-09-06 to ADR 0058 (revised): `parallelMap` only, synchronous, lowered to a pool only for a callback the backend proves allocates nothing, else refused with the reason; no pinned ABI version (the archive was at 31 after the JSON reader, c963d4bc); `parallelReduce` and `parallelSort` are not defined anywhere and are out of scope; simd-json was measured against a scalar Rust reader for JSON and dropped (see `.scratch/json-tape/NOTES.md`).

## Context
`kiln-native-number-abi` is the single source of truth for symbols exported from the native staticlib (`libkiln_native_number_ffi.a`) into compiled native executables. Shipped provider archives and packager manifests assert strictly on `PROVIDER_EXPORTS` and `PROVIDER_ABI_VERSION`.

## Requirements
1. Add `kiln_parallel_map` to `provider_abi_registry!` in `crates/kiln-native-number-abi/src/lib.rs`.
2. Bump `PROVIDER_ABI_VERSION` by one from the tree's current value (31 after c963d4bc).
3. Update provider export assertions in `crates/kiln-native-number-abi/src/lib.rs` (86 -> 89 exports).
4. Update packaged export verification in `crates/kiln-native/src/number_provider.rs`, `scripts/package-native-number-provider.sh`, and `scripts/check-native-number-ffi.sh`.

## Acceptance Criteria
- `mbx test -p kiln-native-number-abi` passes cleanly.
- `bash scripts/check-native-number-ffi.sh` verifies symbol presence and exits 0.
