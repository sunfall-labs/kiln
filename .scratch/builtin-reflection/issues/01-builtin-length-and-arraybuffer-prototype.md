# Issue 01: Built-in Callable Length & ArrayBuffer Prototype

Status: resolved
PRD: `../PRD.md`

## Description

Implement built-in callable arity lookup and ArrayBuffer.prototype linkage in the native C runtime reflection substrate.

### Tasks

1. **ArrayBuffer Prototype Built-in**:
   - Define `KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE` in `crates/native/kiln-backend-c/src/prelude.rs`.
   - Update `builtins.c` for `kiln_c99_builtin_name(KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE) -> "ArrayBuffer"`.
   - Add `{KILN_C99_BUILTIN_ARRAY_BUFFER, KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE}` in `prototypes[]` in `read.c`.
   - In `read.c`, handle `KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE` by delegating to the `ArrayBuffer` member table (`slice`, `constructor`, `is_array_buffer_member`).
   - In `read.c`, add `ArrayBuffer` to `owners[]` mapping prototype back to `ArrayBuffer` constructor.

2. **Built-in Callable Arity (`kiln_c99_builtin_length`)**:
   - Implement `kiln_c99_builtin_length(uint64_t builtin, double *out)` in `builtins.c`.
   - In `read.c`, when reading `"length"` on a built-in callable, check `kiln_c99_builtin_length` and return `kiln_c99_number(length)` if present.

3. **Verification**:
   - Run `builtin-length-refuses.ts` and `builtin-prototype-refuses.ts` on both engines.
   - Run `INSTA_UPDATE=always mbx test -p kiln-backend-c`.
   - Run `mbx test -p kiln-native`.
   - Run `./scripts/native.sh`.
