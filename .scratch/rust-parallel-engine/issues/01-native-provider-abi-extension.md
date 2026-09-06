# Issue 01: Native Provider ABI Extension & Export Verification

Status: planned
Feature: rust-parallel-engine
Governing ADR: ADR 0018, ADR 0025, ADR 0058

## Context
`kiln-native-number-abi` is the single source of truth for symbols exported from the native staticlib (`libkiln_native_number_ffi.a`) into compiled native executables. Shipped provider archives and packager manifests assert strictly on `PROVIDER_EXPORTS` and `PROVIDER_ABI_VERSION`.

## Requirements
1. Add `kiln_parallel_map`, `kiln_parallel_reduce`, and `kiln_parallel_sort` to `provider_abi_registry!` in `crates/kiln-native-number-abi/src/lib.rs`.
2. Bump `PROVIDER_ABI_VERSION` from `29` to `30`.
3. Update provider export assertions in `crates/kiln-native-number-abi/src/lib.rs` (86 -> 89 exports).
4. Update packaged export verification in `crates/kiln-native/src/number_provider.rs`, `scripts/package-native-number-provider.sh`, and `scripts/check-native-number-ffi.sh`.

## Acceptance Criteria
- `mbx test -p kiln-native-number-abi` passes cleanly.
- `bash scripts/check-native-number-ffi.sh` verifies symbol presence and exits 0.
