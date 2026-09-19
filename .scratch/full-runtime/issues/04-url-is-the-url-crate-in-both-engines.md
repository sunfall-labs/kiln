# Issue 04: `URL` is the `url` crate in both engines

Status: ready
Feature: full-runtime
Governing ADR: ADR 0063 (rule 4), ADR 0025 (engines are linked), ADR 0062 (capabilities)

## What is wrong

Natively `new URL("http://user:pw@example.com:8080/a/b?x=1#f")` is an object with `pathname`
and `searchParams` and nothing else: `href`, `origin`, `protocol`, `username`, `password`,
`host`, `hostname`, `port`, `search`, `hash` all read `undefined`, `String(u)` is
`[object Object]`, `JSON.stringify(u)` is `{"pathname":"/a/b","searchParams":{}}`. That is
`web.c`'s "URL subset shared with the interpreter today" (`kiln_c99_url_construct`), a hand
parser that finds `://` and a `?`. The interpreter's `URL` is the `url` crate and matches node on
every property above.

It is the reason `http.get("http://127.0.0.1:1234/x", cb)` stops natively: `node/http.ts` now
reduces a string first argument through `new URL(...)`, reads `protocol`, and gets `undefined`.
Measured 2026-09-19 with `.scratch/full-runtime/probes/url.mjs` and `app2.mjs`; the inventory
did not see it because `URL.prototype` is not a receiver it enumerates.

## Design

Two provider exports in the `realm` group, over the same crate the interpreter links:

- `kiln_url_parse(input, input_len, base, base_len, out, capacity, out_len) -> i32`: the eleven
  components above, NUL-separated, in the order the WHATWG spec lists them; non-zero for an
  invalid URL (the `TypeError` the class throws). Spilled into the caller's buffer the way
  `kiln_node_path_part` is.
- `kiln_url_set(href, href_len, field, value, value_len, out, capacity, out_len) -> i32`: the
  crate's own setters (`set_scheme`, `set_username`, ..., `set_fragment`), so `u.pathname = ".."`
  answers what node answers, including the cases where a setter is ignored.

`PROVIDER_ABI_VERSION` bumps; both signatures join `provider_abi_registry!`; the packager's
export check names them. The `url` crate is already a workspace dependency (`default-features =
false`); `idna` comes with it and is the size to measure -- if it is what ADR 0042's Intl was,
it becomes a capability that only a program reading `URL` links (ADR 0062).

On top: a `URL` class in the runtime bootstrap (TypeScript) holding the eleven components,
getters reading them, setters calling `__kilnUrlSet` and re-reading, `toString`/`toJSON`/
`href`, `searchParams` a `URLSearchParams` over `search` that writes back through the `search`
setter. `web.c`'s constructor goes.

## The one thing that must not regress

Plan step B.3 serves `new URL(req.url, base)` followed by `pathname` or `searchParams.get` from
an object the runtime pre-built for the request (`kiln_c99_active_http_response->url_object`,
`web.c:183`), because a handler that touches `req.url` lightly must allocate nothing. Keep that
object as the fast path -- give it the TS `URL.prototype` and let the other ten getters parse
lazily from `req.url` on first read -- and measure `bench/server` before and after. Anything that
makes `hello`'s handler slower is the wrong shape.

## Done when

`.scratch/full-runtime/probes/url.mjs` prints the same under `kiln run`, `kiln run --native` and
node; `app2.mjs` (the HTTP service with a string-URL client) runs natively; `bench/server`
`hello` and `mixed` are within noise of before.
