# The interpreter's server request is not a Readable

Status: open

In the interpreter, an `http.IncomingMessage` handed to a server's request listener does not
inherit `Readable.prototype`, so `req[Symbol.asyncIterator]` is `undefined` and
`for await (const chunk of req)` throws `TypeError: a undefined is not a function`. The native
backend and node both iterate the body.

```js
import http from "node:http";
http.createServer(async (req, res) => {
  console.log(typeof req[Symbol.asyncIterator]); // node, kiln native: function   interpreter: undefined
  let body = "";
  for await (const chunk of req) body += chunk;  // interpreter: TypeError
  res.end(body);
}).listen(0);
```

The same gap strands a request whose body is read only after it ended: the interpreter's
request emits `end` when the body ends, to no listener, so `data` and `end` listeners added later
(say in a `setTimeout`) never fire and the response never goes out. Node, and the native backend
since the request end waits for a reader, deliver `end` to them.

```js
http.createServer((req, res) => {
  setTimeout(() => {
    req.on("data", () => {});
    req.on("end", () => res.end("ok"));  // interpreter: never called
  }, 20);
}).listen(0);
```

The interpreter's request stream is its own Rust implementation
(crates/interp/kiln-interpreter/src/stream.rs), not the TypeScript one; it also leaves
`req.complete` unset.

`node:http` (crates/semantics/kiln-builtins/src/node/http.ts) re-parents the host's
`__incomingMessagePrototype` onto `Readable.prototype`; the interpreter's host either does not
publish that prototype or allocates requests from a different one.

`conformance/native/http-request-end-deferred.mjs` leaves async iteration and late reads out
until this is fixed, because `scripts/native.sh` treats the interpreter as the oracle.
