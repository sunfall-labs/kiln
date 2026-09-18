Status: closed
Feature: c-prelude-pruning

# 02 - Prune Remaining Dead C Prelude Built-in Implementations

## Context
Following the completion of Phase 5.2 TypeScript built-ins (`crates/kiln-builtins/src/node/*.ts`), several legacy C runtime implementations and dispatch cases were left behind in `crates/kiln-backend-c/src/runtime/`.
These include:
- `node:async_hooks` C remnants (`kiln_c99_new_async_local_storage`, `NODE_ASYNC_RESOURCE*`, `NODE_ASYNC_LOCAL_STORAGE`, `NODE_ALS_*`) in `platform.c`, `foundation.c`, `calls.c`, and `dispatch.c`.
- `node:os` C remnants (`NODE_OS_HOSTNAME`, `NODE_OS_HOMEDIR`, `NODE_OS_TMPDIR`, `NODE_OS_NETWORK_INTERFACES`) in `dispatch.c` and `methods.c`.
- `node:crypto` C remnants (`NODE_CRYPTO_RANDOM_UUID`, `NODE_CRYPTO_CREATE_HASH`) in `dispatch.c`.
- `node:url` C helper remnants (`NODE_URL_FILE_URL_TO_PATH`, `NODE_URL_PATH_TO_FILE_URL`, `NODE_URL_PARSE`) in `dispatch.c`.
- `node:tty` C remnant (`NODE_TTY_ISATTY`) in `dispatch.c`.
- `node:assert` C remnant (`NODE_ASSERT_OK`) in `dispatch.c` and `methods.c`.
- `node:querystring` obsolete parse method name in `methods.c`.

## Tasks
1. [x] **Prune `platform.c`**:
   - Remove `kiln_c99_new_async_local_storage`.
2. [x] **Prune `foundation.c`**:
   - Remove forward declaration `static int kiln_c99_new_async_local_storage(kiln_value *out);`.
3. [x] **Prune `calls.c`**:
   - Remove constructor check for `KILN_C99_METHOD_NODE_ASYNC_LOCAL_STORAGE`.
4. [x] **Prune `dispatch.c`**:
   - Remove `NODE_OS_*` and `NODE_TTY_ISATTY`.
   - Remove `NODE_ASSERT_OK`.
   - Remove `NODE_URL_*`, `NODE_ASYNC_RESOURCE*`, `NODE_ASYNC_LOCAL_STORAGE`, `NODE_ALS_*`, and `NODE_CRYPTO_*`.
5. [x] **Prune `methods.c`**:
   - Remove `case KILN_C99_METHOD_NODE_ASSERT_OK: return "assert";`.
   - Remove `case KILN_C99_METHOD_NODE_QUERYSTRING_PARSE: return "parse";`.
   - Remove `case KILN_C99_METHOD_NODE_OS_HOMEDIR: return "homedir";`.
   - Remove `case KILN_C99_METHOD_NODE_OS_TMPDIR: return "tmpdir";`.
6. [x] **Update `prelude.rs`**:
   - Update `C_STRING_LENGTH_SITES` from 78 to 76 (due to the 2 `strlen` sites in `NODE_OS_HOMEDIR` and `NODE_OS_TMPDIR`).
7. [x] **Verification**:
   - Run `mbx test -p kiln-backend-c` (passed: 161 unit, 36 integration).
   - Update test snapshots (`runtime_prelude_abi_surface.snap`).
   - Run `./scripts/native.sh` (passed: 206 matched, 1 refused, 0 diverged).
   - Run `./target/debug/kiln test conformance/regress --node` (passed: 15/15).
