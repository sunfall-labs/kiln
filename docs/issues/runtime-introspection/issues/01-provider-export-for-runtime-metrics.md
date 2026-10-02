# 01: Reach the runtime and connection numbers from generated C

Status: done

## What

`RealmRuntime::sample_runtime_metrics` and the two HTTP engines' `open_connections` counts are
Rust-private. The dump is assembled in the prelude, so the prelude needs one call that returns all
of them.

## How

- `crates/host/kiln-runtime/src/lib.rs`: add `open_server_connections()` and `open_client_connections()`
  beside the existing `has_open_server_connections`. `requests_in_flight()` is already public.
- `crates/semantics/kiln-native-provider-contract/src/abi.rs`: add
  `(realm, kiln_realm_runtime_report, realm_runtime_report);` to `provider_abi_registry!`, bump
  `PROVIDER_ABI_VERSION` 28 → 29, and update the shape test (86 exports).
- `crates/native/kiln-backend-c/src/lib.rs`: add the `realm_runtime_report` arm to
  `provider_c_declaration!`. Do not hand-write an `extern`.
- `crates/native/kiln-native-provider-ffi/src/lib.rs`: implement it and pin the signature in
  `assert_provider_export!`.

## Shape

```c
int kiln_realm_runtime_report(uint64_t *out_fields, uint64_t capacity, uint64_t *out_count);
```

A slot array rather than a struct: a struct would have to be declared identically in the prelude
and in Rust and kept so by nothing, whereas the count the callee writes tells an older prelude how
much of a newer archive's report it understood. Fields, in order:

| # | Field |
| --- | --- |
| 0 | `live_tasks` |
| 1 | `park_count` |
| 2 | `busy_duration`, microseconds |
| 3 | `elapsed`, microseconds |
| 4 | `global_queue_depth` |
| 5 | `workers` |
| 6 | open server connections |
| 7 | open client connections |
| 8 | requests in flight |

0–5 come from one `sample_runtime_metrics()` call and are interval counters; 6–8 are
instantaneous.

## Done when

`mbx test -p kiln-native-number-abi -p kiln-native-number-ffi -p kiln-backend-c --offline
--no-fail-fast` is green and `scripts/check-native-number-ffi.sh` passes with the new export in the
archive's symbol table.
