# A tagged template's object is rebuilt on every evaluation

**Status:** open. **14 test262 rows**, all in `language/expressions/tagged-template/cache-*`, and
a real-world correctness gap on top of them.

## What is wrong

The specification caches the template object per *site*: every evaluation of one tagged template
hands the tag the same frozen array, for the life of the realm.

```js
function tag(s) { return s; }
function go() { return tag`k`; }
go() === go()      // true in every other engine; false in kiln
```

`ExpressionKind::TemplateStrings` (`crates/kiln-ir/src/compile.rs`) emits instructions that build
the cooked array, build the raw array, freeze both and attach `raw` -- from scratch, every time
the site runs.

This is not only a conformance point. `lit-html`, `graphql-tag` and `styled-components` all key
their caches on template-object identity: a fresh object per evaluation turns their cache into a
leak that never hits. The `conformance/thirdparty` bundles pass today, so nothing in that corpus
depends on it, but that is luck rather than evidence.

The failing tests are `cache-same-site`, `cache-same-site-top-level`,
`cache-different-functions-same-site` and `cache-eval-inner-function` -- the last of which pins
down that the key is the *site*, not the function or the closure.

## Why it is not a small change

The cache has to outlive individual calls and be shared by every closure over the site, so it
cannot be a function-local hidden binding -- those reset per call. It needs program- or
realm-level storage keyed by site, which means either:

- a `LoadTemplateObject { site }` instruction over a per-program table of (cooked, raw), with the
  materialised object cached on first use; or
- a `Constant::Template` that the constant pool materialises lazily and then holds.

Either way both engines must implement it (ADR 0043). The interpreter side is straightforward.
The C backend needs a static slot per site and that slot has to be a GC root -- which is the
`render_gc_roots` machinery that `.scratch/native-module-gc-roots` already records a bug in, so
this change lands on top of that one rather than beside it.

Making the native backend *refuse* tagged templates instead would buy the 14 interpreter rows at
the cost of a capability it currently has, which is the wrong trade and is why this is filed
rather than half-done.


## Tested 2026-09-03: the cheap route does not exist

The obvious way to avoid a new instruction is a module-scope hidden binding per site: check it,
build and store on first use, load on every later one. `declare_import` shows module-scope slots
can be declared (`scopes[0]`, celled so nested functions capture them), so this looked like a
compile.rs-only change reaching both engines for free.

It is not, and the reason is one line: compiling a function does
`std::mem::replace(&mut self.local_count, parameter_count)` and restores it afterwards. While a
function body is being compiled `local_count` belongs to *that function*, so a module slot
allocated from inside one would take the function's counter and collide. A tagged template inside
a function -- which is the common case, and the one the failing tests use -- cannot allocate its
cache slot where it is compiled.

So the cheap route needs a pre-pass that walks for tagged-template sites and allocates their slots
at module scope before any function is compiled. That is real work, and at that point the
per-program table the instruction route wants is no harder and is the design the specification
describes.

Unchanged conclusion, now measured rather than assumed.
