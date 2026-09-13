# PRD: Node Conformance Fixes & Buffer Multi-Encoding Parity

Status: resolved
Branch: `fix/node-conformance-and-buffer-encodings`

## Problem Statement

1. **`conformance/server` failure on `server/s05-express`**:
   - `crypto.createHash().digest("base64")` and `Hmac.digest("base64")` in `crates/kiln-builtins/src/node/crypto.ts` only check `if (encoding === 'hex')` and ignore `'base64'`. They fall back to returning a `Buffer`/`Uint8Array`.
   - Express's `etag` middleware calls `.digest("base64").substring(0, 27)`. When receiving a `Uint8Array`, it throws `TypeError: Uint8Array.substring is not a function`, returning HTTP 500 on all routes.

2. **`conformance/node` failures**:
   - `stream/readable-event`: `Readable.eventNames is not a function` because the `EventEmitter` class in `crates/kiln-builtins/src/node/stream.ts` lacks `eventNames()`.
   - `crypto/random-uuid-url`: `conformance/node/manifest.toml` has stale pre-Phase-5.2 expected stdout (`6e789e...`). Both engines deterministically output `"60f152c3-f6c8-48b8-bc3d-dae5d964aef3\n"` with TypeScript `node:crypto`.

3. **`Buffer.prototype.toString` Multi-Encoding Parity**:
   - `Buffer.prototype.toString("base64")` and `toString("base64url")` throw `unsupported Buffer encoding: base64` in the interpreter and C native runtime.
   - Node.js specifies support for `utf8`, `hex`, `base64`, `base64url`, `latin1`/`binary`, and `ascii`.

## Success Criteria

1. `conformance/server` passes 7/7 (100%).
2. `conformance/node` passes 36/36 (100%).
3. `Buffer.prototype.toString("base64")` and `toString("base64url")` work correctly on both interpreter and native.
4. Clean gate and unit tests (`cargo fmt --check`, `mbx test -p kiln-backend-c`, `mbx test -p kiln-native`, `scripts/native.sh`).
