# Issue 01: Crypto Digest Base64, Stream EventNames, and Manifest Update

Status: closed
PRD: `../PRD.md`

## Description

1. In `crates/kiln-builtins/src/node/crypto.ts`:
   - Implement base64 / base64url encoding in `Hash.prototype.digest(encoding)` and `Hmac.prototype.digest(encoding)`.
   - When `encoding === 'base64'` or `encoding === 'base64url'`, return the encoded string.
   - When `encoding === 'utf8'` or `'utf-8'`, return UTF-8 string.
   - When `encoding === 'latin1'` or `'binary'`, return Latin1 string.
   - When no encoding is passed, return a Buffer.

2. In `crates/kiln-builtins/src/node/stream.ts`:
   - Add `eventNames(): (string | symbol)[]` to the internal `EventEmitter` class.
   - Fix `Readable.prototype.push` and `Readable.prototype.read` state handling and `'readable'` event emission.

3. In `conformance/node/manifest.toml`:
   - Update `expected_stdout` for `node-v24.7.0/crypto/random-uuid-url` to `"60f152c3-f6c8-48b8-bc3d-dae5d964aef3\n"`.

4. Verified:
   - `kiln test conformance/server --node` passes 7/7 (100%).
   - `kiln test conformance/node --node` passes 36/36 (100%).
