# Issue 01: Ambient TypeScript Runtime Bootstrap & Native Intrinsics

Status: resolved
PRD: `../PRD.md`

## Description

Implement `Uint8Array.prototype.toHex`, `Uint8Array.prototype.toBase64`, and JSON replacer/reviver support using the ambient TypeScript runtime bootstrap and minimal native intrinsics.

### Tasks

1. **Native Intrinsics**:
   - Add `KILN_C99_METHOD_UINT8_TO_HEX_RAW` and `KILN_C99_METHOD_UINT8_TO_BASE64_RAW` as raw intrinsic methods.
   - Expose them as `__kilnUint8ToHex` and `__kilnUint8ToBase64` on globals in `globals.c`.
   - In `foundation.c` / `dispatch.c`, implement the raw byte conversion loops.

2. **Prototype Lookup Seam**:
   - In `read.c` and `enumeration.c`, check the prototype's `callable_store` for `KILN_VALUE_TYPED_ARRAY` when `method == 0`.
   - Remove `"toHex"` and `"toBase64"` from `kiln_c99_is_typed_array_member` in `prelude.rs` so they don't refuse.

3. **Ambient TypeScript Bootstrap**:
   - In `NATIVE_BUILTIN_BOOTSTRAP_SOURCE`, capture and delete `__kilnUint8ToHex` and `__kilnUint8ToBase64`.
   - Define `Uint8Array.prototype.toHex` and `Uint8Array.prototype.toBase64(options)`.
   - Wrap `JSON.parse` and `JSON.stringify` to support revivers and replacers in pure TypeScript.

4. **Verification**:
   - Verify `typed-array-codecs-refuse.ts` and `json-roundtrip-edge-refuses.js` pass identically across interpreter and native with zero divergences in `scripts/native.sh`.
