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
