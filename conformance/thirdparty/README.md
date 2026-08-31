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

The thirteen above all passed, and a corpus that passes everything has stopped finding things.
These nine are chosen to be *large machine-generated applications* rather than utility libraries,
and to stress parts of the language the first thirteen never reach: a whole compiler, three more
proxy shapes, two more parser toolchains, runtime code generation, a server renderer, and the
promise machinery. Two pass. Seven fail, and adding them also made a fourteenth -- date-fns --
start failing, which is the most interesting result of the batch.

| probe | what it is, and why it is here | verdict |
|---|---|---|
| `typescript` | the TypeScript 5.9 compiler API: transpile a source string, walk the AST, then type-check through a virtual `CompilerHost`. 9.9 MB, the largest and most language-complete input here | refusal |
| `react-ssr` | `react-dom/server` `renderToString` over a component tree with context, hooks, memo and HTML escaping. The single most common thing a real TypeScript server does | crash |
| `babel` | `@babel/parser` + `traverse` + `generator`: parse, rewrite the AST, print it. A second parser toolchain, built as a chain of `class X extends Y` mixins | crash |
| `postcss` | a CSS tokenizer, mutable AST and stringifier, plus a plugin run through the async processor. A third parser shape: character-code scanning rather than regex | refusal |
| `ajv` | a JSON Schema validator that compiles each schema to JavaScript and hands it to `new Function`. The runtime-code-generation shape every OpenAPI and fastify stack takes | crash |
| `vue-reactivity` | Vue 3's dependency tracking: object, array, collection and readonly proxy handlers, `Reflect` with an explicit receiver, `effectScope` | **wrong answer** |
| `async-concurrency` | p-queue / p-map / p-limit / p-retry / p-timeout and IxJS: `Promise.any`, `allSettled`, `withResolvers`, Promise subclassing, `AbortSignal`, `Symbol.asyncIterator` | **wrong answer** + refusal |
| `mobx` | a third proxy shape: `defineProperty` accessor pairs under a proxy, transactions, `observe`/`intercept`, lazy computeds | pass |
| `rxjs` | observables, schedulers and Promise interop in both directions, `from(asyncIterable)`, subjects, `retry` | pass |

### What each one found

Ordered the way this corpus is meant to be read: a wrong answer outranks a refusal, because a
program that gets a wrong number does not know it needs to stop.

- **date-fns started failing without its probe changing.** It prints `2020-02-15` where node prints
  `2020-02-14` -- one day off. `getTimezoneOffset()` answers 0 and
  `Intl.DateTimeFormat().resolvedOptions().timeZone` answers `"UTC"` whatever the host is set to,
  so every local-time getter returns the UTC value and `format` renders the wrong day anywhere
  west of Greenwich. It is the worst shape a bug can take for a suite: correct under CI, where TZ
  is UTC, and wrong on every developer machine and most production hosts.
  `findings/local-timezone.js`.
- **vue-reactivity found that a proxied object accepts exactly one write.** `Reflect.set(target,
  key, value, receiver)` where the receiver already has an own data property must call
  `receiver.[[DefineOwnProperty]]` with a descriptor holding only `[[Value]]`; kiln creates a
  fresh data property instead, so the first write turns the property non-writable,
  non-enumerable and non-configurable and every later write returns false and is silently dropped.
  Vue's whole handler is built on that call, so `state.count` froze at 1 and three separate
  assertions quietly read stale values. `findings/reflect-set-receiver.js`.
- **async-concurrency found four things at once**, which is why its probe runs each block through a
  `step` helper -- a probe that stops at the first gap only ever finds one gap.
  `e.name` for p-timeout's `TimeoutError` answers `"Error"`: a class field named `name` on an
  `Error` subclass is overwritten whenever the subclass declares its own constructor, so
  `if (e.name === "TimeoutError")` takes the wrong branch (`findings/error-subclass-name-field.js`).
  `Promise.any`'s rejection carries `.errors` but is not an `AggregateError`, because
  `AggregateError` does not exist. `class P extends Promise {}` inherits no statics, so `P.resolve`
  and `P.all` are `undefined`. And `AbortController` is missing, which took out both the abort
  block and IxJS. `findings/promise-subclass-and-globals.js`.
- **typescript and postcss both died on the same cause**: the `--node` builtin modules are partial
  stubs whose exports are not enumerable. `Object.keys(require("path"))` is 0 against node's 17, so
  every `{ ...require("path") }` re-export shim yields nothing, and individual members are simply
  absent -- `os.platform` (which is where the TypeScript compiler's `getNodeSystem` stops) and
  `path.dirname` (which is where postcss's `PreviousMap` stops).
  `findings/node-builtin-surface.js`.
- **ajv and babel both hit the interpreter's step budget.** `ExecutionLimits::default` sets
  `max_steps` to 1,000,000, there is no flag to raise it, and `ajv.compile({ type: "integer" })` --
  the smallest schema there is -- exceeds it on its own. Babel parses `const a = 1;` fine and only
  fails on the real probe, so the budget is the whole of what stops both.
- **react-ssr found an interpreter invariant failure.** `import "react-dom/server"` alone, with no
  render, aborts with `local slot 460 does not hold a captured cell`: the compiler emitted a
  capture read for a slot it never boxed. Not a JavaScript error a program could catch -- the
  process stops with an internal message.
- **The bundling itself found two more.** Every ESM-for-node bundle esbuild, tsup or rollup emits
  carries `createRequire(import.meta.url)`; kiln refuses `import.meta` in the frontend, and
  `createRequire` reached any other way fails at run time. That is why the four probes needing Node
  builtins are bundled as CommonJS instead of ESM. And once they are CommonJS, `require` resolves
  only the specifiers a static pass already recorded: the `var __require = typeof require !==
  "undefined" ? require : ...` alias every bundler emits throws `Cannot find module 'path'`, as
  does any computed specifier. The same probe shows `__require === require` is `false`, so the
  `require` binding is not identical to itself across two reads. `findings/require-alias.js`,
  `findings/import-meta-and-createrequire.mjs`.
- **mobx and rxjs passed byte for byte**, which is a real result: mobx is as proxy-heavy as Vue and
  rxjs is the most promise-dense input in the corpus.

### Two more, found by probing rather than by a library

Neither blocks an input here yet, and both are the kind of thing a real program would hit without
noticing:

- `Object.getOwnPropertySymbols` always answers `[]` and `Reflect.ownKeys` omits symbol keys, even
  though the properties exist and read back correctly. Every faithful-copy routine -- deep clone,
  deep merge, a decorator re-wrapping an object -- silently drops symbol-keyed data.
  `findings/own-property-symbols.js`.
- `Function.prototype.toString` answers `[Function 0]` rather than source text, so the
  `/native code/` test every polyfill uses to decide whether an intrinsic is real gets a confident
  wrong answer. `findings/function-tostring.js`.

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
