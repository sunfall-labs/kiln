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
What each found is kept because it is the record of what only real code finds:

- **marked** found that `encodeURI` and its three siblings did not exist. It sanitises every href
  through `encodeURI` inside a `try`, so the `ReferenceError` was swallowed and every link in a
  document silently rendered as plain text -- the worst shape a missing global can take. It also
  found that `String.replace` with a *string* pattern ignored `$&`, `` $` ``, `$'` and `$$` in the
  replacement, where the regexp path handled them.
- **acorn** parses JavaScript, so it exercises the language against itself: heavy regexes,
  classes, string scanning, a hand-written state machine. It passed on its first run, which is
  the only entry here that did.
- **lodash** found that `typeof capturedName` answered `"undefined"` for a name belonging to an
  *enclosing function* -- the check jumped to module scope and skipped everything between. Every
  CommonJS shim a bundler emits writes `typeof exports == "object"` inside a nested function, so
  lodash decided it was in a browser, published itself on the global object, and left
  `module.exports` empty. It also found that `Object.getOwnPropertyNames` ignored a function's
  own properties, which is how the same shims copy a module whose exports *are* a function.
- **uuid** was the one that wanted a *feature* rather than a fix: `Uint8Array`, `ArrayBuffer` and
  the legacy `unescape`. It hashes a name into bytes, so it exercises the byte-level half of the
  language nothing else here touches.
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

## The heavy set

The thirteen above passed on their first full runs, and a corpus that passes everything has stopped finding things.
These nine are chosen to be *large machine-generated applications* rather than utility libraries,
and to stress parts of the language the first thirteen never reach: a whole compiler, three more
proxy shapes, two more parser toolchains, runtime code generation, a server renderer, and the
promise machinery. When they were added, five passed and four failed, each naming one missing
thing rather than a mixture. Current verdicts come from `scripts/thirdparty.sh`; the history
below is what each probe found, with its status at the time it was written.

Four of them -- `typescript`, `babel`, `postcss`, `react-ssr` -- are bundled as CommonJS for the
*node* platform (see *Building the heavy set*), so `scripts/thirdparty.sh` runs those four under
`--node`. Running one of them without the adapter it was built for reports "require is not
defined", which says nothing about the library.

| probe | what it is, and why it is here |
|---|---|
| `typescript` | the TypeScript 5.9 compiler API: transpile a source string, walk the AST, then type-check through a virtual `CompilerHost`. 9.9 MB, the largest and most language-complete input here |
| `react-ssr` | `react-dom/server` `renderToString` over a component tree with context, hooks, memo and HTML escaping. The single most common thing a real TypeScript server does |
| `babel` | `@babel/parser` + `traverse` + `generator`: parse, rewrite the AST, print it. A second parser toolchain, built as a chain of `class X extends Y` mixins |
| `postcss` | a CSS tokenizer, mutable AST and stringifier, plus a plugin run through the async processor. A third parser shape: character-code scanning rather than regex |
| `ajv` | a JSON Schema validator that compiles each schema to JavaScript and hands it to `new Function`. The runtime-code-generation shape every OpenAPI and fastify stack takes |
| `vue-reactivity` | Vue 3's dependency tracking: object, array, collection and readonly proxy handlers, `Reflect` with an explicit receiver, `effectScope` |
| `async-concurrency` | p-queue / p-map / p-limit / p-retry / p-timeout and IxJS: `Promise.any`, `allSettled`, `withResolvers`, Promise subclassing, `AbortSignal`, `Symbol.asyncIterator` |
| `mobx` | a third proxy shape: `defineProperty` accessor pairs under a proxy, transactions, `observe`/`intercept`, lazy computeds |
| `rxjs` | observables, schedulers and Promise interop in both directions, `from(asyncIterable)`, subjects, `retry` |

### What each one found

Statuses (*fixed*, *open*) are as of when each probe was written and may have moved since; the `findings/` reproductions are the way to check one. Ordered the way this corpus is meant to be read: a wrong answer outranks a refusal, because a
program that gets a wrong number does not know it needs to stop. Each entry records what the
probe found and where that finding now stands.

- **vue-reactivity found that a proxied object accepts exactly one write.** `Reflect.set(target,
  key, value, receiver)` where the receiver already has an own data property must call
  `receiver.[[DefineOwnProperty]]` with a descriptor holding only `[[Value]]`; kiln completed that
  descriptor first, so the first write turned the property non-writable, non-enumerable and
  non-configurable and every later write was refused. Vue's whole handler is built on that call,
  so `state.count` froze at 1 and three separate assertions quietly read stale values.
  *Fixed*; the probe now matches node byte for byte. `findings/reflect-set-receiver.js`.
- **async-concurrency found four things at once**, which is why its probe runs each block through a
  `step` helper -- a probe that stops at the first gap only ever finds one gap.
  `e.name` for p-timeout's `TimeoutError` answered `"Error"`: a class field named `name` on an
  `Error` subclass was overwritten whenever the subclass declared its own constructor, so
  `if (e.name === "TimeoutError")` took the wrong branch (*fixed*, and it was two bugs -- an own
  `name` on every error instance, and a derived class initialising its fields before `super()`
  returned rather than after; `findings/error-subclass-name-field.js`).
  `Promise.any`'s rejection carried `.errors` but was not an `AggregateError`, because
  `AggregateError` did not exist (*fixed*). `class P extends Promise {}` inherited no statics, so
  `P.resolve` and `P.all` were `undefined` (*fixed*, and its factories now answer promises of the
  subclass). `AbortController` is still missing, which is the whole of what stops this probe
  today. `findings/promise-subclass-and-globals.js`.
- **date-fns started failing without its probe changing.** It prints `2020-02-15` where node prints
  `2020-02-14` -- one day off. `getTimezoneOffset()` answers 0 and
  `Intl.DateTimeFormat().resolvedOptions().timeZone` answers `"UTC"` whatever the host is set to,
  so every local-time getter returns the UTC value and `format` renders the wrong day anywhere
  west of Greenwich. *Open, and open deliberately*: ADR 0031 makes the realm's timezone a
  capability whose default is UTC and rejects "read the host by default" by name, so the default
  cannot change here. What is missing is the other half -- `Date`'s local getters and setters
  ignore a realm zone that is not UTC, which ADR 0031's own status lists as outstanding migration
  work. Until they honour it, a `--timezone` flag would be a lie for `Date`.
  `findings/local-timezone.js`.
- **typescript and postcss both died on the same cause**: the `--node` builtin modules are partial
  stubs whose exports are not enumerable. `Object.keys(require("path"))` is 0 against node's 17, so
  every `{ ...require("path") }` re-export shim yields nothing, and individual members are simply
  absent -- `os.platform` (which is where the TypeScript compiler's `getNodeSystem` stops) and
  `path.dirname` (which is where postcss's `PreviousMap` stops). *Open when
  written*; see `findings/node-builtin-surface.js`.
- **ajv and babel both hit the interpreter's step budget.** `ExecutionLimits::default` set
  `max_steps` to 1,000,000 and there was no flag to raise it, so `ajv.compile({ type: "integer" })`
  -- the smallest schema there is -- was refused on its own. *Fixed*: `kiln run` is unmetered and
  `--max-steps` imposes a bound for a caller that wants one; the conformance corpus keeps the
  budget, which is where the guarantee is actually needed. Both probes pass.
- **react-ssr found an interpreter invariant failure.** `import "react-dom/server"` alone, with no
  render, aborted with `local slot 460 does not hold a captured cell`: a `var` declared inside one
  arm of an `if` had its cell created inside that arm, and the other arm stored into a box that did
  not exist. Not a JavaScript error a program could catch -- the process stopped with an internal
  message. *Fixed*; the probe now reaches an ordinary catchable `TypeError` for the globals it
  still wants -- `MessageChannel`, `TextEncoder`, `ReadableStream`, and `async_hooks`'s
  `AsyncLocalStorage`.
- **The bundling itself found two more.** Every ESM-for-node bundle esbuild, tsup or rollup emits
  carries `createRequire(import.meta.url)`; kiln refuses `import.meta` in the frontend, and
  `createRequire` reached any other way fails at run time. That is why the four probes needing Node
  builtins are bundled as CommonJS instead of ESM. And once they are CommonJS, `require` resolves
  only the specifiers a static pass already recorded: the `var __require = typeof require !==
  "undefined" ? require : ...` alias every bundler emits throws `Cannot find module 'path'`, as
  does any computed specifier. The same probe shows `__require === require` is `false`, so the
  `require` binding is not identical to itself across two reads. *Open*. `findings/require-alias.js`,
  `findings/import-meta-and-createrequire.mjs`.
- **mobx and rxjs passed byte for byte**, which is a real result: mobx is as proxy-heavy as Vue and
  rxjs is the most promise-dense input in the corpus.

### Two more, found by probing rather than by a library

Neither blocked an input here, and both are the kind of thing a real program would hit without
noticing:

- `Object.getOwnPropertySymbols` answered `[]` and `Reflect.ownKeys` omitted symbol keys, even
  though the properties were there and read back correctly. Every faithful-copy routine -- deep
  clone, deep merge, a decorator re-wrapping an object -- silently dropped symbol-keyed data.
  *Fixed* in both engines. `findings/own-property-symbols.js`.
- `Function.prototype.toString` answers `[Function 0]` rather than source text, so the
  `/native code/` test every polyfill uses to decide whether an intrinsic is real gets a confident
  wrong answer. *Fixed* in both engines: each function carries a span into its unit's retained
  source, and a built-in answers the `[native code]` form. `findings/function-tostring.js`.
- The native backend answers a built-in's `length` with a refusal where the interpreter answers
  the number. Every other member of every callable shape now agrees between the two engines;
  this one wants a shared arity table rather than a second copy of the interpreter's.
  *Open*. `findings/callable-builtin-length.js`.
- Kiln compiles CommonJS as strict where node runs it sloppy -- but only for the binding rules,
  not for a refused write. Every bundle in this corpus is CommonJS, so no real program here can
  reach the sloppy-mode `this` rule at all, which is why fixing that rule moved 201 Test262
  variants and nothing else. *Open*. `findings/commonjs-runs-strict.js`.

### findings/

One minimal reproduction per cause, each carrying the observed node and kiln output in its header.
They are not wired into any runner; they are the record of what the bundles above found, in a form
small enough to read. Run one with
`kiln --one-shot --node run conformance/thirdparty/findings/<name>.js`.

### Building the heavy set

`build.sh` builds all twenty-two probes. Nine of them need packages the original thirteen did not,
and four -- `typescript`, `babel`, `postcss`, `react-ssr` -- are bundled as CommonJS for the node
platform rather than ESM for the neutral one, for the `import.meta` reason above. `typescript` is
pinned to 5.9: TypeScript 7 is the Go port and ships a native binary, so there is no JavaScript in
it to run. The three largest bundles are gitignored on the same reasoning as zod's and yaml's.
