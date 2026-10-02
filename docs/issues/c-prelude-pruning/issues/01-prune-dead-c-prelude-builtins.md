Status: closed
Feature: c-prelude-pruning

# 01 - Prune Dead C Prelude Built-in Implementations

## Tasks
1. [x] **Prune `node_support.c`**:
   - Remove `kiln_c99_node_dns_lookup`.
   - Remove `kiln_c99_node_channel` and `kiln_c99_node_diagnostics`.
   - Remove `kiln_c99_node_util`.
2. [x] **Prune `foundation.c`**:
   - Remove forward prototypes for `kiln_c99_node_util`, `kiln_c99_node_dns_lookup`, `kiln_c99_node_path`, and `kiln_c99_node_querystring_parse`.
   - Remove `kiln_c99_node_path_answer` and `kiln_c99_node_path`.
3. [x] **Prune `web.c`**:
   - Remove `kiln_c99_node_querystring_parse`.
4. [x] **Prune `prelude_declarations.h`**:
   - Remove extern declarations for `kiln_c99_node_dns_lookup` and `kiln_c99_node_util`.
5. [x] **Prune `dispatch.c`**:
   - Remove handlers for `NODE_UTIL`, `NODE_DIAGNOSTICS`, `NODE_HTTPS_CREATE_SERVER`, `NODE_HTTP2_CREATE_*`, `NODE_DNS_LOOKUP`, `NODE_PATH`, and `NODE_QUERYSTRING_PARSE`.
6. [x] **Update `prelude.rs`**:
   - Remove `kiln_c99_node_diagnostics` and `kiln_c99_node_util` from `ABSENT_ANSWER_SITES`.
   - Update `C_STRING_LENGTH_SITES` from 79 to 78.
7. [x] **Update `tests.rs`**:
   - Replace `kiln_c99_node_util(..., INHERITS)` in `tests.rs` with standard prototype chaining (`kiln_c99_set_prototype`).
   - Replace `KILN_C99_METHOD_NODE_UTIL_NOOP` in `executable_array_from_maps_an_array_like_object` with closure mapper.
   - Remove obsolete `executable_node_support_facades_are_deterministic_and_callback_driven` test.
8. [x] **Update `kiln-liveness` and `facts.rs`**:
   - Map `Path`, `Util`, `UtilTypes`, `DiagnosticsChannel` to `0` in `kiln-liveness`.
9. [x] **Verification**:
   - Run `mbx test -p kiln-backend-c` (161 unit tests, 36 emitted_c tests pass).
   - Update snapshots (`runtime_prelude_abi_surface.snap`).
   - Run `./scripts/native.sh` (206 matched, 1 expected shadow-realm refusal, 0 diverged).
