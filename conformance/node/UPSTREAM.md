# Node compatibility corpus

This directory pins an executable slice of Node.js `fs/promises` behavior to Node tag `v24.7.0`,
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

Node mode currently has one supported execution path: the bytecode interpreter selected by
`kiln test --node`. The native backend does not accept or silently emulate Node mode yet; when its
private continuation ABI grows asynchronous HostOps, this corpus becomes the shared interpreter /
native gate. Run today's pinned gate with:

```sh
mbx run -p kiln-cli -- --one-shot test conformance/node --node
```

Ordinary deterministic conformance continues to omit `--node`, so it never observes ambient I/O.

## Compatibility inventory

`node-compat.json` is the Deno-style compatibility inventory. It pins the upstream repository,
tag, commit, and suite directory, then assigns every selected upstream path an explicit
expectation. Passing entries are CI claims; expected failures and skips require reasons. Platform
exclusions also require reasons. Every selected file carries its SHA-256, so validation fails closed
if a path disappears or its bytes differ from the pinned Node revision.

The initial inventory intentionally follows the real-server dependency chain: filesystem cases
already passing, then Buffer, EventEmitter, streams, TCP resources, and HTTP. It records cluster as
out of the current scope rather than allowing the absence of a test to look like compatibility.
