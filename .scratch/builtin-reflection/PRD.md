# PRD: Built-in Reflection Parity (Arities & ArrayBuffer.prototype)

Status: proposed
Branch: `feat/builtin-reflection-parity`

## Problem Statement

The native C backend currently has two reflection refusals in `scripts/native.sh`:
1. `conformance/native/builtin-length-refuses.ts`: Reading `.length` on a built-in callable (such as `Array.length`) refuses because the native runtime lacks a built-in arity reflection table, even though `kiln-interpreter` already defines the full specification arity in `native_length()`.
2. `conformance/native/builtin-prototype-refuses.ts`: Reading `ArrayBuffer.prototype` refuses because `ArrayBuffer` was omitted from the `prototypes[]` lookup table in `read.c`. While `ArrayBuffer` instances already model `slice`, `byteLength`, and unbuilt member refusals, the prototype object itself has no builtin constant ID or linkage.

## Architecture

1. **Built-in Callable Arity (`kiln_c99_builtin_length`)**:
   - In `crates/kiln-backend-c/src/runtime/builtins.c`, implement `kiln_c99_builtin_length(uint64_t builtin, double *out)` reflecting standard ECMAScript constructor and function arities (matching `crates/kiln-interpreter/src/lib.rs:native_length()`).
   - In `read.c`, when reading `"length"` on a non-namespace `KILN_VALUE_BUILTIN`, consult `kiln_c99_builtin_length()`. If found, return `kiln_c99_number(length)`. Unknown/unmodeled callables continue to safely refuse.

2. **ArrayBuffer Prototype Linkage (`KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE`)**:
   - In `crates/kiln-backend-c/src/prelude.rs`, define `KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE` and register it.
   - In `read.c`, add `{KILN_C99_BUILTIN_ARRAY_BUFFER, KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE}` to the `prototypes[]` table, and route prototype property reads through the existing `ArrayBuffer` member table.
   - Ensure `typeof ArrayBuffer.prototype === "object"` and constructor is `ArrayBuffer`.

## Success Criteria

1. `conformance/native/builtin-length-refuses.ts` passes with identical output to the interpreter (`Array.length 1`).
2. `conformance/native/builtin-prototype-refuses.ts` passes with identical output to the interpreter (`ArrayBuffer.prototype object`).
3. `scripts/native.sh` refusal backlog drops from 3 to 1 (only `shadow-realm` remaining). Zero divergences.
4. Clean checks on `cargo fmt --check` and `mbx test -p kiln-backend-c` / `kiln-native`.
