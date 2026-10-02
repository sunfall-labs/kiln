# Issue 08: `Buffer.prototype` exists in neither engine

Status: fixed 2026-09-19
Feature: full-runtime

Found by the app-shaped probe `events-streams.mjs` (2026-09-19): every other line of an
EventEmitter + stream + Buffer program is node-identical in both engines now, and the Buffer
line is not:

```js
const b = Buffer.from("héllo wörld", "utf8");
b.equals(Buffer.from(b));   // interpreter: TypeError: Uint8Array.equals is not a function
b.readUInt16BE(0);          // native: TypeError: a undefined is not a function
Buffer.alloc(3, 1).toJSON() // neither
typeof Buffer.prototype     // "undefined" in both; node: "object"
```

What each engine has today:

* Interpreter: a Buffer is a `Uint8Array` with `toString` hijacked for *every* `Uint8Array`
  (`crates/interp/kiln-interpreter/src/buffer.rs`); `Buffer.isBuffer(new Uint8Array(1))` is `true`;
  `Buffer.from(text, encoding)` ignores the encoding (`Buffer.from("ff", "hex")` is the two
  bytes of `"ff"`); the statics are `from`, `alloc`, `isBuffer`, `byteLength`, `concat`.
* Native: a typed array with `is_buffer` set; `toString` is the one instance method
  (`runtime/read.c`); the same five statics in C; `Buffer.prototype` refuses as a built-in
  callable's own `prototype`.

Node's `Buffer.prototype` has about sixty members: `read{U,}Int{8,16,32}{LE,BE}`,
`readBig{U,}Int64{LE,BE}`, `readFloat/Double{LE,BE}` and the `write*` twins, `equals`,
`compare`, `copy`, `fill` (with encodings), `indexOf`/`lastIndexOf`/`includes` over a string,
number or buffer needle, `slice`/`subarray` returning Buffers, `toJSON`, `toString(encoding,
start, end)`, `write(string, offset, length, encoding)`, `swap16/32/64`, `toLocaleString`.

## The shape of the fix

One TypeScript prototype for both engines, per ADR 0005 / 0063 rule 4: build
`Buffer.prototype = Object.create(Uint8Array.prototype)` with the members above in a shared
bootstrap (`crates/semantics/kiln-builtins/src/web.ts` is what both engines run in node mode today),
over the encode/decode intrinsics the engines already have for `toString`.

What each engine needs underneath:

* A Buffer *brand* on the instance. Native has `is_buffer`; the interpreter needs the same
  field on `TypedArrayData` (fourteen construction sites, one Buffer factory:
  `node_bytes_value`), and `Buffer.isBuffer` reads it instead of "is a Uint8Array".
* The read path for a branded view consults `Buffer.prototype` before
  `Uint8Array.prototype`, and `Object.getPrototypeOf(buf)` answers it. Native: the
  `KILN_VALUE_TYPED_ARRAY` arm of `read.c` before the concrete-prototype store; interpreter:
  the `TypedArray` arm before `table_step`. Neither engine can take
  `Object.setPrototypeOf(view, P)` as the mechanism -- native refuses it on a typed array,
  and it would not reach the Buffers Rust and C build (`fs.readFileSync`, streams).
* `Buffer.from(text, encoding)` honours the encoding in both engines; `Buffer.isBuffer` is
  the brand; `slice`/`subarray`/`Buffer.concat` results are branded.

`class B extends Uint8Array` is not the route: natively a subclass instance is an object
(tag 9) and `subarray` on it refuses as "an array method this backend has not implemented",
which is its own bug and a smaller one than this.

## What landed

`crates/semantics/kiln-builtins/src/buffer.ts`, appended to `WEB_BOOTSTRAP_SOURCE` so both engines run it
in Node mode: `Buffer.prototype = Object.create(Uint8Array.prototype)` with the sixty-odd
members, the hex/base64/latin1/utf16le codecs in TypeScript, and `from`/`alloc`/`concat`/
`byteLength`/`compare`/`isEncoding`/`copyBytesFrom` over the engines' statics. The decoder is
captured from `__kilnBufferToString` without a call, so a Node-mode program that never
reaches Buffer boots with the group pruned; a program that does retains `Math`/`Number` too
(`kiln-liveness`).

Interpreter: `ObjectData.buffer_brand` on the view's slot (`node_bytes_value` sets it;
`TextEncoder.encode` answers a plain `Uint8Array`), `Buffer.isBuffer` reads it, a branded
view's member read and `Object.getPrototypeOf`/`instanceof` go through `Buffer.prototype`
(recorded when the store write lands), `JSON.stringify` and `ToPrimitive` reach its `toJSON`
and `toString`, and `Buffer.from(arrayBuffer, offset, length)` shares bytes.

Native: `kiln_c99_buffer_prototype` reads the store; the `KILN_VALUE_TYPED_ARRAY` read arm,
`getPrototypeOf`, `JSON.stringify` and `Array.prototype.join` consult it for `is_buffer` views;
`Buffer.from` takes the ArrayBuffer window; `Buffer.byteLength` counts UTF-8 bytes.

Probes: `conformance/native/buffer-prototype.mjs`, `buffer-edges.mjs`, `app-events-streams.mjs`,
all node-identical in both engines.

## Acceptance

`docs/issues/full-runtime/probes/events-streams.mjs` node-identical in both engines, added to
`conformance/native/` as `app-events-streams.mjs`; the node column of the inventory for
`Buffer.prototype` reads zero missing.
