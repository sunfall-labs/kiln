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

`node:http` (crates/semantics/kiln-builtins/src/node/http.ts) re-parents the host's
`__incomingMessagePrototype` onto `Readable.prototype`; the interpreter's host either does not
publish that prototype or allocates requests from a different one.

`conformance/native/http-request-end-deferred.mjs` leaves async iteration out until this is
fixed, because `scripts/native.sh` treats the interpreter as the oracle.
