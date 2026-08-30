# Third-party bundles

Real libraries, bundled with esbuild, run as a correctness probe. They are here for the reason
the Effect corpus is: they are large, machine-generated, and written against the whole language
rather than against what kiln happens to implement, so they find gaps that a hand-written probe
never would.

Each of these found something on its first run, and every one was a *silent* wrong answer rather
than a missing feature:

- **date-fns** found that `s[0]` on a string was `undefined`, that a `Date` had no setters, and
  that no intrinsic value answered `constructor`.
- **immer** found that `globalThis` was a fresh object on every read, so anything stored on it
  vanished and `globalThis === globalThis` was false. It is the one that does not pass yet. `Proxy` exists now
  and immer gets much further with it -- drafts are created, writes go through -- but its finalisation leaves a nested
  draft in the result where node replaces it with a plain object.
- **semver** found that `Map.prototype.values()` and `.keys()` yielded whole `[key, value]`
  pairs, so `[...map.values()]` was an array of pairs; that `Map`/`Set` had no `forEach`; and that
  a regexp's `toString` was `Error.prototype.toString`, so every regexp stringified as "Error".
- **mustache** found that `RegExp.prototype` did not exist -- `RegExp.prototype.test.call(re, s)`
  is how a library avoids a monkey-patched `test` -- and that `String.prototype.substring` and
  `substr` were missing entirely.
- **js-yaml** is not passing. It reads a flat mapping and a sequence correctly and fails on a
  *nested* one: `load("a:\n  b: 1\n")` throws "incomplete mapping pair in event stream" where
  node returns `{a: {b: 1}}`. That two-line input is the whole repro. The primitives its scanner
  uses -- `charCodeAt`, prefix and postfix increments of a member expression, `Object.create(null)`,
  `Map` with object keys, optional chaining on null -- all agree with node when probed directly,
  so whatever diverges is further in.
- **zod** found that `Object.defineProperty` with a data descriptor ran an *inherited setter*
  instead of defining an own property, that a named function expression could not call itself,
  and that `Array(n)` and `Object(x)` were not callable without `new`. The first of those was an
  infinite recursion that took the Rust stack with it, which is the one failure a compiler must
  never have.

Regenerate with `conformance/thirdparty/build.sh`, which needs `npm` and the esbuild in
`conformance/effect/node_modules`. The small bundles are committed -- a fixed input that does not
move when a library releases is the point -- and the large ones are not, on the same reasoning
that keeps the Effect bundles out: 700 KB of machine-generated near-duplicate does not belong in
a history. `.gitignore` draws the line by name.
