# Node compatibility corpus

This directory pins an executable slice of Node.js compatibility behavior to Node tag `v24.7.0`,
commit `d795edb56765d4ec743bcda5a0c1452bed74de0c`. The two primary source tests are:

- `test/parallel/test-fs-promises-readfile-empty.js`, SHA-256
  `10247f1876bcae623c792ea78f890f6ba2682299636e3075c468c52e7d1e2870`
- `test/parallel/test-fs-promises-writefile-typedarray.js`, SHA-256
  `a8cb5228c030b21dd560feb7362391fa217a3f9c06ecb2bd624103ce73151395`

Node's files depend on its private `test/common`, fixture, temporary-directory, `assert`, `Buffer`,
CommonJS, and synchronous-fs harnesses. The adapted files replace only that harness: paths point
at committed fixtures, `process.getBuiltinModule('node:fs/promises')` supplies the same built-in
namespace, and a successful assertion prints `pass`. The API actions and assertions are split into
small cases so the manifest records exactly which behavior passed rather than letting one early
failure hide the rest.

The stat, ENOENT, namespace-identity, overwrite/truncate, Promise-chain, and concurrency cases
extend the same upstream API slice with Node-shaped assertions needed to cover Kiln's first
Adapter boundary. They are derived compatibility probes rather than byte-for-byte upstream files;
the manifest labels every exercised behavior so the supported surface is explicit.

Node mode's complete corpus currently runs through the bytecode interpreter selected by
`kiln test --node`. Script cases use the direct program entrypoint; `.mjs` cases deliberately use
the production Node resolver, typed builtin graph, linker, shared realm session, and Tokio Adapter.
That second path is where server compatibility is gated, so a test-only loader cannot make the
corpus greener than a real module graph. Native Node mode is also explicit (`kiln run --node
--native`) and uses the same typed resolver plus active-realm asynchronous HostOps, but advertises
only the native materializers that have vertical coverage (`node:events`, `node:net`, and the
server subset of `node:http`). Unsupported builtins still fail before emission. The inventory
becomes a shared interpreter/native gate case by case as each adapted test's complete surface is
materialized. Run the complete interpreted pinned gate with:

```sh
mbx run -p kiln-cli -- --one-shot test conformance/node --node
```

Ordinary deterministic conformance continues to omit `--node`, so it never observes ambient I/O.

## Compatibility inventory

`node-compat.json` is the Deno-style compatibility inventory. It pins the upstream repository,
tag, commit, and suite directory, then assigns every selected upstream path an explicit
expectation. Passing entries are CI claims and must name a complete executable adaptation. Partial
adaptations remain expected failures, but name their runnable cases and the remaining upstream
assertions in the reason. Expected failures and skips require reasons. Platform
exclusions also require reasons. Every selected file carries its SHA-256, so validation fails closed
if a path disappears or its bytes differ from the pinned Node revision.

The inventory follows the real-server dependency chain: filesystem cases pass completely;
EventEmitter, TCP, HTTP, and Buffer now have executable partial coverage; generic streams remain an
explicit gap. Buffer's core allocation/concat/UTF-8 behavior is exercised independently and by the
request-body server probe. The HTTP coverage is self-contained in both directions: one case exercises a paused /
resumed chunked response from `node:http` to `node:net`; the canonical server probe uses
`node:http` for both server and client, including response decoding and `setEncoding`. Tokio owns
both I/O paths and the scheduler/liveness decision. Cluster stays out of scope explicitly rather than
letting an absent test look like compatibility.

Sequential client requests are also an executable adaptation of the pinned HTTP server slice.
They prove that response, body, finish, and error events route by a realm-scoped request identity
instead of a connection-local constant. The inventory deliberately continues to name Agent
connection reuse as missing until the interpreter and generated-native bindings both acquire an
idle same-origin connection from the realm-owned Agent rather than opening one TCP resource per
request.
