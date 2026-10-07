Status: closed
Feature: open-world-speculation

# 02 - Static Built-in Predicates Specialization

## Goals
1. Inline `Array.isArray(x)` in `call_method.rs` under `kiln_c99_builtin_store_epoch == 0` for direct array pointer tag checks (`KILN_TAG_BUFFER_PREFIX` + `KILN_GC_ARRAY` -> `true`) and primitives (number, boolean, nullish, string, bigint, symbol, closure -> `false`). Other objects (Proxies, revoked proxies, `Array.prototype`) fall back to `kiln_c99_call_method`.
2. Inline `Number.isFinite`, `Number.isNaN`, `Number.isInteger`, `Number.isSafeInteger` in `call_method.rs` under `kiln_c99_builtin_store_epoch == 0` evaluating on unboxed numbers directly without call overhead.
3. In `body.rs`, mark arguments to `Number.*` predicates as unboxed if already proven numbers, and arguments to `Array.isArray` as unboxed if proven numbers.
4. In `clones.rs`, allow `Array.isArray` and `Number.*` predicates in restartable fast clones.
5. In `direct.rs`, include `Array.isArray` and `Number.*` predicates in `intrinsic_results` as `Proven::Boolean`.

## Acceptance Criteria
- Full test pass in `kiln-backend-c`. (Passed: 160 unit tests, 36 snapshot tests)
- Native parity suite matches across interpreter and native (`scripts/native.sh`). (Passed: 206 matched, 0 diverged)
- Thirdparty bundles pass (`mise run gate-quick`). (Passed: 19 matched, 0 diverged)
- Executable tests added: `executable_static_builtin_predicates_array_is_array`, `executable_static_builtin_predicates_number`.
