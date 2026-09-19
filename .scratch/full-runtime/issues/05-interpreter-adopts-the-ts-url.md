# Issue 05: the interpreter serves `URL` from the same TypeScript

Status: fixed 2026-09-19 (Node mode); the `URLSearchParams` global stays held back
Feature: full-runtime
Governing ADR: ADR 0063 rule 1, ADR 0005

With issue 04 landed the native backend's `URL` is byte-identical to node on
`.scratch/full-runtime/probes/url2.mjs`; the interpreter's is not: setters are ignored
(`u.pathname = "/other"` leaves `href` alone), `URLSearchParams` has `get`/`has`/`getAll`/
`toString` and no iteration, `size`, `append`, `set`, `delete` or `sort`, `instanceof URL` is
false, and `console.log(u)` prints `{}`. Node is the target of both engines, so this is the same
debt the native side just paid, and the payment is the same file: `crates/kiln-builtins/src/url.ts`
over two intrinsics the interpreter exposes from its own `url` crate the way the native backend
does from the provider.

The `URLSearchParams` global is held back natively until then, so the two engines' global
lists stay equal; `url.ts` publishes it in one line once the interpreter can too.

Done when the interpreter evaluates `url.ts` at realm setup with `__kilnUrlParse`/`__kilnUrlSet`
answered from `crates/kiln-interpreter/src/url.rs`, `probes/url2.mjs` matches node under
`kiln run`, and `conformance/native/url-surface.mjs` grows the setter and iteration lines back.

## What landed

`kiln-url` holds the record and the setters (moved out of the provider FFI, which spills them
now); the interpreter publishes `__kilnUrlParse`/`__kilnUrlSet` over it and `__kilnUrlFast`/
`__kilnUrlAdopt` as no-ops, runs `URL_BOOTSTRAP_SOURCE` ahead of the web bootstrap in Node
mode, and resolves the bare `URL` name to the class the bootstrap assigned rather than the
Rust constructor. `node:url` exports that same class (`require("node:url").URL === URL`), where
it had carried a third implementation of its own. `url.mjs` and `url2.mjs` are node-identical in
both engines.

Left open: outside Node mode the interpreter has no realm bootstrap, so it keeps the Rust
`URL` there and the `URLSearchParams` global stays unpublished in both engines (it appears in
both after `import "node:url"`); `fetch("data:...")` fails in both engines.
