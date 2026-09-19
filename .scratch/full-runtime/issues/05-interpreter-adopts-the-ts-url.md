# Issue 05: the interpreter serves `URL` from the same TypeScript

Status: ready
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
