# JavaScript conformance corpus

**Nothing in this directory runs.** Kiln cannot execute JavaScript yet, so no test here has ever
produced a result. `./status.sh` says so at length and refuses to report a pass rate. Read this
directory as a specification of behaviour kiln will have to get right, and as a set of fixtures
waiting for an interpreter — not as a suite that is currently green.

## Where this came from

A previous project made [porffor](https://github.com/CanadaHonk/porffor), an ahead-of-time
JavaScript-to-WebAssembly-to-C compiler, run real [Effect-TS](https://effect.website) programs
end to end, including interruption. Doing that meant finding and fixing about two dozen genuine
compiler bugs across eleven branches. This corpus is what that work distilled down to: the tests
that were worth keeping once the bugs were closed.

    source repository:  porffor-effect-findings
    source commit:      bfdd2b0  "porffor/Effect: findings, and the three regression gates"
    ported:             2026-08-29

It is worth having here because it is a **semantics** corpus, not a porffor-implementation
corpus. Not one test asserts anything about how porffor works. Every one asserts what JavaScript
does, and kiln has to produce the same answers. The porffor-specific material — precompile
passes, comptime flags, codegen internals, wasm lowering — was deliberately left behind; none of
it transfers and all of it would be noise.

What did transfer, and is written down below, is the part that was expensive to learn: which
JavaScript semantics a real library actually depends on, and how the bugs were caught.

## Layout

    conformance/
      manifest.toml      every test: source path, expected stdout, feature tags, status
      status.sh          inventory and per-test status; refuses to imply success
      README.md          this file
      guard/             20 standalone probes (.js), copied verbatim, plus their generator
      effect/
        src/             12 Effect-TS programs (.ts), copied verbatim
        build.sh         bundles src/ into single self-contained files
        package.json     pins effect@4.0.0-rc.112 and esbuild@0.27.7
        package-lock.json
        fixtures/        one committed bundle, as a large parser input
        bundle/          generated, gitignored

## The two halves

### `guard/` — 20 probes, one behaviour each

Small, standalone, no imports, no harness. Each probe was written to catch one specific defect,
and each is deliberately minimal so that a failure names the defect rather than a region of the
compiler. `manifest.toml` records what each one catches in its `catches` field.

Between them they cover: object spread and rest keeping symbol keys; arrow functions inheriting
`arguments`; class field initialisers capturing the enclosing scope; generic `for..of` through a
user `@@iterator`; the `yield*` delegation protocol with two-way `next(v)`; `@@iterator` existing
as a real, identity-stable property on Array, Set, Map, String and generators; timer and
microtask ordering; `try`/`finally` on all four abrupt exit paths; `Error` subclassing;
`Error` `cause` and `stack`; iterator close on `break`; static and instance private-name brand
checks; Set mutation during iteration; generator `.return()` unwinding; builtin statics reached
through a variable; a prototype set on a function; and real derived-constructor semantics over
`Array` and `Map`.

Two probes are worth singling out because their shape encodes a lesson:

- **`g06-array-proto-iterator.js`** is a single line and references nothing but the symbol. An
  earlier version also touched `[].values`, which silently forced the array prototype to
  materialise and hid the fact that the fix only worked when something *else* in the same program
  had already triggered it. A probe that accidentally satisfies its own precondition tests
  nothing.
- **`g06b-array-iterator-identity.js`** checks that `@@iterator` is *present* (`Symbol.iterator in
  []`) and is the *same function object* as `Array.prototype.values`. Behaving correctly is not
  enough; Effect's `Effect.forEach` tests `Symbol.iterator in v` and refuses arrays if the answer
  is false.

A note on faithfulness: the probe `.js` files are copied byte for byte and were **not** edited.
The task that commissioned this port described each probe as carrying a comment naming the bug it
catches. That is not the case — in the source corpus only three of the twenty files carry any
comment at all, and the rationale for all twenty lives in `guard/gen.mjs`, the script that
generated them. Rather than write comments into the probes, `gen.mjs` is carried over unmodified
alongside them and the rationale is recorded per test in `manifest.toml`.

### `effect/src/` — 12 real programs

Twelve Effect-TS programs, `01-core` through `12-service-layer`, in increasing order of
difficulty. These are not probes. Each is a small but genuine program that pulls in a whole
library at once, and they exist because probes and broad conformance suites both miss things that
real programs hit — see *Methodology* below.

| program | what it adds |
|---|---|
| `01-core` | `succeed`, `map`, `flatMap`, `pipe`, `runSync`, `runPromise` |
| `02-gen` | `Effect.gen` and `yield*` — the first program needing the iterator protocol |
| `03-ref` | `Ref.make` / `update` / `get` through `runPromise` |
| `04-all-foreach` | `Effect.all`, sequential `forEach` |
| `05-concurrency` | bounded and unbounded concurrency, results in input order |
| `06-sleep` | `Effect.sleep` — the first program needing a real event loop |
| `07-interruption` | `race` and `timeout`, which interrupt by mutating a Set mid-iteration |
| `08-scope` | `Scope`, `acquireRelease`, release observed through a closure |
| `09-fiber` | `forkChild` + `Fiber.join`; the module-semantics test |
| `10-tagged-error` | `Data.TaggedError` + `catchTag` |
| `11-stream` | `Stream.fromIterable` / `map` / `runCollect` |
| `12-service-layer` | `Context.Service` + `Layer` + gen + concurrency + timers |

`12-service-layer` is the one the source project treated as its definition of done: dependency
injection, generators, concurrency and timers in a single program.

## What real Effect programs require of a JavaScript engine

This is the part that is most directly useful to kiln, because it is a requirements list that was
established empirically rather than guessed. Each item below is something a real Effect program
stopped working without.

**Iteration.** A generic iterator protocol — `GetMethod(obj, @@iterator)`, then drive
`.next()` / `done` / `value` — shared by `for..of`, array spread, `Array.from`, and `yield*`. Not
a type-tag switch over built-in types. Built-in prototypes must carry `@@iterator` as a real,
observable, identity-stable property, because libraries feature-detect with `Symbol.iterator in
v`. Iterator close (`return()`) must run on early exit. `yield*` must implement the full
delegation protocol, including passing values back in through `.next(v)`.

**Symbols.** Object spread and rest must carry symbol-keyed own properties (while `Object.keys`
must still exclude them). Effect builds every prototype by spreading one proto into another, and
`[Symbol.iterator]` on those protos is what makes an Effect yieldable at all. A symbol-keyed
member read must find an inherited property with no help from any other construct in the program.

**Closures and scoping.** Arrow functions inherit `arguments` from the enclosing function —
`dual()`, which defines nearly every data-last/data-first Effect API, is built on this. Class
instance-field initialisers capture the enclosing scope; `Data.TaggedError` is literally
`tag => class Base extends Error { _tag = tag }`.

**Classes and prototypes.** Real derived-constructor semantics: `this` binds to the result of
`super()`, so `class X extends Array` is an actual Array. `class X extends Error` produces a
working Error — `instanceof Error` true, `.message` set, `String(e)` correct — which means errors
must be ordinary objects with a real prototype chain, not a distinct primitive type tag. A
prototype set on a *function* must be honoured for lookups and inherited through the static side,
because `Context.Service` does `Object.setPrototypeOf(KeyClass, ServiceProto)` and the yieldable
`@@iterator` lives on that prototype. Private-name brand checks must be correct in both
directions: a static private called through a subclass throws, an instance private stays
reachable from an inherited public method.

**Errors.** `.stack` present and a string; the `cause` option preserved by identity;
`Error.captureStackTrace` present. Effect reads `.stack` for span and trace rendering across
fifteen modules and calls `captureStackTrace` in sixteen.

**Control flow.** `finally` runs on every exit path — normal completion, `return`, a propagating
throw, and `break`/`continue` out of a loop. This one is silent when wrong rather than loud, which
makes it the most dangerous: every JS-level resource cleanup and every generator-`.return()`
interruption path just quietly does not happen. `generator.return(v)` must unwind the suspended
body so its `finally` runs, then complete with `{ value: v, done: true }` — that is the mechanism
a fiber runtime interrupts with.

**Collections.** Set and Map iteration must tolerate deletion during iteration. `race` and
`timeout` interrupt their children by deleting each one from a Set as they iterate it, so this
single behaviour gates all of interruption.

**Globals and statics.** A constructor's statics must be reachable through a variable holding the
constructor, not only through its global name — Effect does `const Array = globalThis.Array` and
then `Array.isArray(...)`.

**The event loop.** `setTimeout` with forwarded arguments, `clearTimeout`, `setInterval` cleared
from inside its own callback, `queueMicrotask`, and correct ordering between microtasks and
timers. Feature detection on `"setImmediate" in globalThis` picks the scheduler.

**Module semantics.** Top-level `var` in a module must not become a property of the global
object. This matters even for the bundles, which have no `import` or `export` left in them after
esbuild: Effect's scheduler is chosen by `"setImmediate" in globalThis ? … : setTimeout` wrapped
around its own top-level binding, and script semantics take the wrong branch.

Two things were *not* required, and are worth knowing so they are not over-built:
array-destructuring of a generic iterable (`const [a, b] = customIterable`) and iterator close on
`return` / `throw` / labelled jump. Effect needs neither, and in the source project both had bad
cost/benefit — the first turns a loud TypeError into a hang on an infinite iterable.

## Methodology — the part worth keeping

The source project ran three gates, and the reason all three existed is the single most
transferable thing here.

    guard/run.sh        20 probes, one behaviour each         ~6s
    effect/run.sh       12 real Effect programs               ~60s
    guard/ab.sh         per-test test262 A/B against a base   ~25m/side

Each of the three bugs introduced *during* the fixing work was caught by a **different** gate:

1. Closing an iterator that was never opened. Passed all 20 probes. Showed no test262
   regression. Only a real Effect program caught it.
2. Two colliding `setTimeout` definitions that broke `clearTimeout` on its own and broke one
   build configuration entirely. Caught by a separate investigation pass, not by any of the
   three suites.
3. Deleting a getter broke `e.constructor` for runtime-thrown errors. Caught by the probes.

**A broad conformance suite and a set of small probes both miss things that real programs hit.**
A probe is small enough to be understood, which is also small enough to accidentally satisfy its
own precondition. A conformance suite is broad, which means it is broad in the directions its
authors thought of. A real program composes features in combinations nobody wrote a test for —
which is exactly why `design.md` §15 step 6's interpreter/native equivalence check matters: it
runs whatever the developer is actually running, in both backends, and compares. That check is
the same kind of gate as `effect/run.sh`, and it is load-bearing for the same reason.

Three smaller lessons, each learned the expensive way:

- **A probe that references anything beyond its subject may test nothing.** See
  `g06-array-proto-iterator.js` above.
- **An aggregate pass count is not a regression check.** "7,595 → 7,651 passes" hides a test that
  started failing behind two that started passing. Only a per-test diff names the regression.
  Relatedly, the source project's test262 runner would, when handed a file subset, diff that
  subset against the *full* baseline, report every absent pass as a regression, and then
  overwrite the baseline. Whatever kiln's runner does, a partial run must never be able to write
  a full baseline.
- **Single-test checks on a loaded machine produce false negatives.** A timing-sensitive test
  that "produced no output" had simply not finished. Re-verify on an idle machine before
  believing a result.

## How expected output was obtained

Every `expected_stdout` in `manifest.toml` was **observed**, not transcribed. It is what
`node v25.9.0` actually printed on the porting machine, captured with stdout and stderr merged
and stored byte for byte including the trailing newline.

- **Guard probes.** The source corpus stores no expected output at all — its `guard/run.sh`
  diffs the compiler's output against a live `node` run every time — so there was nothing to
  copy and the values had to be produced. Each probe was run with `node <probe>.js`. All twenty
  exit 0 and write nothing to stderr.
- **Effect programs.** Each program carries an `// expect:` comment, but those are lossy: they
  join output lines with `" | "`, and `12-service-layer`'s own output contains a literal `" | "`,
  so the comment cannot be parsed back into lines mechanically. The bundles were therefore built
  with `build.sh` and run under `node`, and the observed stdout recorded.

  One disagreement surfaced. `07-interruption`'s comment reads
  `// expect: timeout threw TimeoutError | race fast`, but the observed order is the reverse —
  `race fast` first — which is also what the code says should happen, since the race resolves on
  a 2 ms sleep and the timeout does not fire until 5 ms. Confirmed identical over ten
  consecutive runs. The observation is what `manifest.toml` records; the comment is left
  unedited in the source file, and the discrepancy is noted there in
  `expect_comment_disagrees`.

Every recorded value was re-verified against a fresh `node` run after the manifest was written:
32 of 32 matched byte for byte.

Output stability was checked deliberately for the timing-sensitive tests
(`guard/g07-timers-order`, `effect/06-sleep`, `effect/07-interruption`, `effect/12-service-layer`).
All were identical across repeated runs. That is a property of node's scheduler and not a
guarantee: under `design.md` §11 these should eventually be driven by the mock event driver's
virtual time rather than by real sleeping, and the tests tagged `timers` or `event-loop` are
precisely the ones that will need it.

## Regenerating the Effect bundles

Porffor had no module resolution, so each program had to be bundled into a single file with the
whole Effect library inlined. Kiln will want a real module loader, but the bundles stay useful for
a different reason: each is around 200 KB of real, machine-generated JavaScript, which is a much
better parser and frontend input than anything written by hand.

```sh
cd conformance/effect
npm ci          # effect@4.0.0-rc.112, esbuild@0.27.7, both pinned exactly
./build.sh      # writes bundle/*.js
```

`NODE_MODULES` and `ESBUILD` override the defaults if you would rather point at an existing tree.
The adapted `build.sh` differs from the original in exactly one respect: the two hardcoded
absolute paths to a particular laptop are gone, replaced by resolution against this directory's
own `node_modules`.

`bundle/` is gitignored. Twelve bundles are 2.5 MB of near-duplicate output, because each one
inlines the same library; `package-lock.json` plus `build.sh` reproduces them byte for byte,
which was verified by building twice and diffing.

### The one committed bundle

`effect/fixtures/12-service-layer.bundle.js` — 209 KB — **is** committed. The reasoning, since
vendoring a fifth of a megabyte of third-party output deserves an argument:

- Kiln needs a large, real frontend and parser input, and it needs one now. `bench/` already
  exists and grows scenarios as the compiler does. Hand-written benchmark input is not
  representative; a minified bundle is not either, because minification erases exactly the
  shapes — long identifier chains, deep nesting, thousands of small functions — that stress a
  parser. This is unminified bundler output, which is what real input looks like.
- A benchmark must not need a network or a Node toolchain to run. Regenerating requires
  `npm ci`, which requires both.
- `12-service-layer` is the right one of the twelve. It is the program the whole effort
  culminated in, it exercises the most library surface, and at 209 KB it is representative of the
  set. (`11-stream` is 377 KB and unrepresentative; `01-core` at 198 KB is the same size but
  much thinner.)
- The cost is bounded and auditable: one file, regenerable byte for byte, with its size and
  SHA-256 recorded in `manifest.toml` so drift is detectable.

The other eleven earn nothing that this one does not, and would add 2.3 MB of near-duplicates.

## Conventions, and where they diverge from the rest of the repo

**Feature tags.** `manifest.toml` follows `docs/design.md` §9, which asks each test to declare
what it exercises. The `features` list is the field a changed-test selector keys on, so the tags
are meant to be accurate rather than decorative: each names a JavaScript semantic the test would
actually stop passing without. They map directly onto
`kiln_test_selector::TestMetadata::feature()`. §9's `ir` field is deliberately absent — kiln has
no IR operations yet, so every value would be invented, and an invented tag in a selector input is
worse than a missing one, because it makes the selector confidently wrong instead of
conservatively broad.

**A manifest rather than frontmatter.** `tests/conformance/smoke/*.js` declares its metadata in
test262-style `/*--- … ---*/` frontmatter inside each file. That is the better convention and
should stay the default for tests kiln writes itself. It could not be used here: the guard probes
are copied byte for byte, and `g06-array-proto-iterator.js` in particular is a single line whose
whole point is that it contains nothing else. An external manifest keeps the fixtures pristine.
When these tests become runnable, a loader for `manifest.toml` should sit next to the frontmatter
parser rather than replacing it.

**`status.sh` sources `scripts/lib.sh`.** The narrow reason is style — every script in this
repository that prints for a human does so through `kiln_say`, and duplicating that here would
start a second convention. The wider reason is that `kiln_block` is what mirrors output into the
GitHub job summary, and the job summary is the single place where a false "0 failures" would do
the most damage, so the script's honesty is worth exactly as much as its reaching that surface.
The cost is a real dependency from `conformance/` on `scripts/`, but it is one-directional and
covers three tiny helpers.

**`status.sh` will not report a pass rate.** All 32 tests are `not-yet-runnable`. "32 tests, 0
failures" is true arithmetic and a false claim: it says the corpus passed when the corpus did not
run. This is the same failure mode `kiln_report_has_measurements` in `scripts/lib.sh` already
refuses to allow for an empty benchmark report — "a green tick on an empty measurement is worse
than no measurement at all" — and it applies with more force to a conformance suite, which is the
thing people most want to read a green tick off. `status.sh --gate` therefore exits non-zero for
exactly as long as nothing can run, so wiring the corpus into CI early produces an honest red
rather than a hollow green.

## What was not ported

- **`repros/`, 100 files.** The raw survey that produced `REPORT.md` — every probe written while
  mapping the gap, including dead ends, duplicates, and files whose names only mean something
  next to `REPORT.md`'s prose. `guard/` *is* the distillation of `repros/`: twenty files that
  survived because they each catch something, against a hundred that were exploratory. They also
  carry no expected output and no per-file rationale, so porting them would add a hundred
  fixtures nobody could act on. The findings they support are summarised above instead.
- **`guard/run.sh` and `effect/run.sh`.** Both invoke the `porf` binary and diff against a live
  `node` run. Kiln has no runner to substitute, and a script that cannot run is worse than none.
  Their method is worth keeping and is recorded here: strip ANSI escapes, compare merged
  stdout+stderr byte for byte against `node`, run the cases in parallel because almost all the
  wall time is compilation. When kiln has an interpreter, that is the shape to rebuild.
- **`guard/ab.sh`.** A test262 A/B harness specific to porffor's test262 runner and its
  `--json-results` flag. `crates/kiln-test262` is the right home for the equivalent. The lesson
  it encodes — a partial run must never write a full baseline — is recorded above.
- **`GOAL.md`, `STACK.md`, `REPORT.md`.** Read for this port and distilled into the two sections
  above. What is left in them is porffor implementation detail: which file each fix landed in,
  which comptime flag ran when, wasm and C lowering, and a branch-by-branch history of a stack of
  pull requests against a different compiler. None of it transfers.
- **`node_modules`.** Replaced by `package.json` + `package-lock.json`.
- **Eleven of the twelve bundles.** See *The one committed bundle* above.
