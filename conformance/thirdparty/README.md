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
  vanished and `globalThis === globalThis` was false. It later found the larger one: `===` was
  false for a proxy, and for a date, a regexp, a promise, a bound function and a BigInt. Every
  one of those was unequal to itself.
- **semver** found that `Map.prototype.values()` and `.keys()` yielded whole `[key, value]`
  pairs, so `[...map.values()]` was an array of pairs; that `Map`/`Set` had no `forEach`; and that
  a regexp's `toString` was `Error.prototype.toString`, so every regexp stringified as "Error".
- **mustache** found that `RegExp.prototype` did not exist -- `RegExp.prototype.test.call(re, s)`
  is how a library avoids a monkey-patched `test` -- and that `String.prototype.substring` and
  `substr` were missing entirely.
- **js-yaml** found that assigning `array.length` did nothing. Its parser speculates and rewinds,
  and rewinding truncates the event list with `events.length = n`; the write was stored as an
  ordinary property called "length" and the speculative events stayed, so a nested mapping
  composed against a duplicated event stream. Only nested mappings failed, which is what made it
  worth chasing: a flat one and a sequence both parsed.
All six pass. What each found is kept because it is the record of what only real code finds:

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
