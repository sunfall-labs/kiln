# Issue 02: Buffer.prototype.toString Base64 & Base64url Support

Status: closed
PRD: `../PRD.md`

## Description

1. In `crates/interp/kiln-interpreter/src/buffer.rs`:
   - Added `"base64"` and `"base64url"` arms to `buffer_to_string`, dispatching to `crate::to_base64_text(&bytes, url, omit_padding)`.

2. In `crates/native/kiln-backend-c/src/runtime/node_support.c`:
   - Added `"base64"` and `"base64url"` decoding support in `KILN_C99_METHOD_BUFFER_TO_STRING` delegating to `kiln_c99_uint8_to_base64_raw`.

3. Tests:
   - Added base64 and base64url tests to `conformance/native/node-buffer-namespace.cjs`.
   - Verified interpreter and native parity via `./scripts/native.sh` (154 passed, 0 failed).
