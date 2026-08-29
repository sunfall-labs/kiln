# Language coverage: where the interpreter is, and the order to grow it

This document turns "grow the interpreter" into a backlog. It is written against measurements,
not against intent: every claim about what kiln does today came from running the CLI, and the
commands are given so the numbers can be re-taken when they stop being true.

    export PATH="$HOME/.local/share/mise/shims:$PATH"
    mbx build -p kiln-cli
    ./target/debug/kiln --one-shot run <file>

The corpus under test is `conformance/` — 20 guard probes and 12 Effect programs, described in
`conformance/README.md`. Its manifest records all 32 as `not-yet-runnable`, and `status.sh`
refuses to report a pass rate for exactly that reason. Nothing below changes that; this document
exists to say what it would take to change it, and in what order.

Measurements were taken on 2026-08-29, against `node v25.9.0` as the reference.

---

## 1. Where the surface actually is

`examples/hello.ts` runs. It is the only runnable example program committed to the repository
(`examples/` otherwise holds `kiln-bench`'s generated workloads, which are not committed). It is
seven lines, and this is all of it:

```ts
let total: number = 0;
while (total < 3) {
  total += 1;
}

const state = total === 3 ? "ready" : "wrong";
console.log("kiln", state, total);
```

`./target/debug/kiln --one-shot run examples/hello.ts` prints `kiln ready 3`, which is correct.

### What that does imply

It is a real end-to-end pipeline, not a demo. Oxc parses TypeScript, `frontend-oxc` lowers to
owned HIR with the type annotation stripped, `kiln-ir` compiles that to stack bytecode, and
`kiln-interpreter` executes it against `HostOps::WriteStdout`, which `kiln-runtime` implements.
Five crates and a versioned ABI are load-bearing for those seven lines.

Some of what it proves is genuinely reusable:

- **Jump patching works.** `while_statement` and the conditional expression both emit a forward
  jump with a placeholder and patch it once the target is known (`Compiler::emit_jump` /
  `patch_jump`). Every control-flow feature still to come — `for`, `break`, `continue`,
  `try`/`finally`, `switch` — is built on that same primitive, and it is already correct
  including the loop's backward edge.
- **Local slots are stable across a back edge.** The compiler deliberately never recycles slots,
  with the reason recorded at the field: a loop can jump back into a block, so execution must not
  depend on the compiler's scope bookkeeping.
- **Coercions are being taken seriously.** `+` string-concatenates when either side is a string;
  relational comparison of two strings goes through UTF-16 code units rather than Rust's scalar
  ordering, with the spec reason in a comment.
- **Refusals are diagnostics, not panics.** `compile()` accumulates `CompileDiagnostic`s and
  returns all of them. Running a probe gives you its whole blocker list in one shot rather than
  one error at a time, which is what makes §3 of this document cheap to produce.

### What it does not imply

Almost everything else. Stated concretely:

- **There is no heap.** `kiln_interpreter::Value` has five variants — `Undefined`, `Null`,
  `Boolean`, `Number(f64)`, `String(String)` — and `HostValue` in the ABI mirrors it. There is no
  allocation, no object identity, and `strict_equal` has no reference case at all. Nothing in the
  corpus can run until that changes.
- **There are no call frames.** The VM has one `locals: Vec<Value>`, one instruction pointer, and
  `Instruction::Return` terminates the program. There is no call stack, no call instruction, and
  no closure representation. A function declaration is refused by the frontend before the IR ever
  sees it.
- **`console.log` is not a value.** `Compiler::call` pattern-matches the callee shape
  `console.log` at compile time and lowers it to a `HostCall`, guarded by
  `self.resolve("console").is_none()` so a local named `console` correctly disables it (verified:
  `let console = 1; console.log("x")` is refused). `console.error` is refused. There is no
  `console` object, no property lookup, and no callable value anywhere in the system.
- **Member access exists in the HIR but not in the IR.** `ExpressionKind::StaticMember` is lowered
  by the frontend and then unconditionally refused by `kiln-ir` with ``unsupported member access
  `.p` ``. The one exception is the `console.log` shape above.
- **There are no exceptions.** No `throw`, no `try`. `ExecutionError` is a VM fault — step limit,
  stack underflow, bad jump — and is not a JavaScript exception; there is no mechanism by which
  JavaScript code could observe or catch one.
- **The operator surface is narrower than it looks.** Verified refused: `typeof`, `instanceof`,
  `in`, `==`/`!=`, `++`/`--`. Verified accepted: the twelve binary operators in `BinaryOp`, three
  unary operators, `&&`/`||`/`??`, and six compound assignments.
- **Statements: `let`/`const`/`var`, expression statements, blocks, `if`, `while`, empty.** That
  is the entire list. `for`, `do`-`while`, `break`, `continue`, `return`, `switch`, `throw`,
  `try`, function declarations and class declarations are each named and refused.
- **Output already diverges from node.** `console.log(-0)` prints `0` under kiln and `-0` under
  node, because `number_string` collapses `value == 0.0` to `"0"` before formatting. That is one
  line to fix, and it is called out here because it is the kind of thing that gets discovered by a
  conformance run rather than by reading.

One more piece of good news, because it is easy to miss: **the frontend already parses 209 KB of
real bundler output.** Running the committed fixture
`conformance/effect/fixtures/12-service-layer.bundle.js` produces 727 diagnostics, and every one
of them is a lowering refusal — there is not a single parse error in the set. The parser is not
the bottleneck. (727 badly undercounts the real gap: a refused `function` declaration collapses
its entire body into one diagnostic, and 36 of those 727 are function declarations covering most
of the file.)

---

## 2. The refusal vocabulary, measured

Running all 20 guard probes and tallying the diagnostics gives the actual shape of the boundary.
"probes" counts how many distinct probes emit that diagnostic at least once; "total" counts
occurrences across the corpus.

| diagnostic | probes | total |
|---|---:|---:|
| `only console.log(...) calls are supported` | 16 | 39 |
| `unsupported expression: expression` | 12 | 21 |
| `unsupported statement: function declaration` | 6 | 9 |
| `unsupported statement: class declaration` | 5 | 11 |
| ``unsupported expression: unary operator `typeof` `` | 5 | 13 |
| `unsupported statement: for statement` | 3 | 3 |
| ``unsupported member access `.<name>` `` (10 distinct names) | 9 | 15 |
| `unsupported expression: computed property access` | 2 | 3 |
| ``unsupported expression: binary operator `instanceof` `` | 2 | 5 |
| `unsupported statement: try statement` | 2 | 2 |
| `unresolved identifier` (`Array`, `Object`, `JSON`, `K`, `Sub`) | 2 | 5 |
| ``unsupported expression: binary operator `in` `` | 1 | 1 |
| `destructuring bindings are not supported yet` | 1 | 1 |

Two lines in that table matter more than the rest.

`only console.log(...) calls are supported` is the top line by a wide margin, and it is the one
refusal that is unavoidable: 16 of 20 probes hit it. It is not really about `console.log` — it is
the message you get for *any* call that is not the one hardcoded shape, so it covers user
function calls, constructor calls, builtin statics and method calls all at once.

`unsupported expression: expression` is the frontend's catch-all, and it is doing too much work.
Verified by single-line probes, it is emitted for object literals, array literals, template
literals, `new`, arrow functions, and `++`. When acting on this backlog, splitting that one
feature name into real ones is cheap and would make the next round of measurement far sharper.
(That is a change inside `crates/frontend-oxc`, which this document does not touch.)

---

## 3. Per-probe requirements

Each probe's full blocker set, taken from the CLI rather than inferred. Because `compile()`
accumulates diagnostics, this is the complete set per probe, not merely the first refusal.

| probe | what it needs beyond today |
|---|---|
| g01 spread-symbols | `Symbol()` call, object literal with computed keys, object spread, object **rest** destructuring, symbol-keyed property reads, `Object.keys`, `Array.prototype.join` |
| g02 arrow-arguments | function declarations, `return`, arrow functions, the `arguments` object with arrow inheritance, array literal, `Array.prototype.map` taking a callback, `.join` |
| g03 class-field-closure | arrow function, **class expression** with instance-field initialisers capturing an enclosing parameter, `new`, property read |
| g04 generic-forof | object literal with a computed-symbol method, arrow, `for..of` over a user `@@iterator`, iterable spread, `.join` |
| g05 yieldstar-protocol | classes, generator function, `yield*` delegation with two-way `next(v)`, user `@@iterator`, property reads, `.toUpperCase`, `.push`, `.join` |
| g06 array-proto-iterator | array literal, computed member access, `Symbol.iterator`, `Array.prototype[@@iterator]` as a real function **value**, `typeof` |
| g06b array-iterator-identity | as g06, plus the `in` operator and `[][Symbol.iterator] === [].values` — function object **identity** |
| g07 timers-order | `setTimeout` with forwarded args, `clearTimeout`, `setInterval` cleared from its own callback, `queueMicrotask`, and an event loop that orders them |
| g08 finally-paths | function declarations, `return`, `throw`, `try`/`catch`/`finally` on all four abrupt paths, `for`, `break`, `continue`, `new Error`, `.message` |
| g09 error-subclass | `class extends Error`, `super(m)`, derived-constructor `this`, `instanceof`, `.message`/`.name`, `String(e)`, `globalThis` |
| g10 builtin-statics | `Array`/`Object`/`JSON` as ordinary bindings, statics reached **through a variable**, `typeof`, array and object literals, `.join` |
| g11 proto-on-function | object literal with a computed-symbol method, function declaration, `Object.setPrototypeOf` on a **function**, `class Sub extends K`, static inheritance, `typeof` |
| g12 private-not-inherited | four class declarations, static and instance private names, brand checks in both directions, `try`/`catch`, `e.constructor.name` |
| g13 set-mutation | `new Set()`, `add`/`delete`/`size`, object literals, `for..of` over a Set with deletion mid-iteration, `.push`/`.length`/`.join` |
| g14 generator-return-finally | generator function, `try`/`finally` inside it, `.next()`, `.return(v)` unwinding through `finally`, `.value`, `JSON.stringify` |
| g15 error-stack-cause | `new Error(msg)`, options object with `cause`, `.stack` as a string, `.cause` preserved **by identity**, `typeof Error.captureStackTrace` |
| g16 array-from-iterable | object literal with a computed-symbol **generator** method, `Array.from` via `@@iterator`, iterable spread, `.join` |
| g17 set-map-iterator | `new Set()`/`new Map()`, `@@iterator` on Set, Map, String and generator objects, generator function, `typeof` |
| g18 iterator-close | array literal, object literal with a computed-symbol method, `for..of`, `break`, IteratorClose calling `return()`, `.push`/`.join` |
| g19 subclass-array-map | `class extends Array` and `class extends Map`, derived construction over exotic builtins, `push`/`set`/`get`, `instanceof`, `.length` |

Grouped, the corpus needs eleven things: **a heap**, **functions and calls**, **objects,
prototypes and member access**, **arrays and symbols**, **spread/rest/destructuring**, **full
control flow including exceptions**, **the iterator protocol**, **Set and Map**, **classes and
derived constructors**, **generators**, and **timers with an event loop**. That list is the ladder
in §4, in a different order.

---

## 4. The implementation sequence

Each step states what it contains, why it sits where it does, whether its position is **forced**
(something below it cannot work otherwise) or merely **convenient** (a defensible choice among
several), and which probes it should turn green.

The predicted deltas are the point of the exercise. Each is a falsifiable claim made before the
work, so a step that lands and produces a different number has told you something — usually that
a probe depends on something nobody wrote down. Being wrong here is cheap. Not predicting is not.

### Step 1 — A heap, functions, calls, closures

Object identity and allocation in `Value`; call frames and a call stack in the VM; function
declarations, function expressions and arrows in the frontend; parameters, `return`, lexical
environments with capture, `this`, and the `arguments` object with arrow inheritance.

**Forced, absolutely.** Nothing else on the ladder is reachable without it. A property whose value
is a method is a function object. An iterator is an object with a callable `next`. A class method
is a function. A getter is a call. And under design.md §5 every builtin is itself a JS function,
so even "just add `Array.prototype.join`" is a function.

The heap belongs here rather than as its own step because closures need heap-allocated
environments — splitting them would produce a rung that cannot be finished.

**Predicted unlocks: none. Running total 0/20.**

That zero is worth dwelling on. It is the only step on the ladder with no probe payoff, and if
progress is measured by pass count alone, this step looks like failure. It is not: it is the unit
every later step is denominated in. Note also that `arguments` is folded in here — it is function
semantics, not a separate feature, even though only g02 tests it.

### Step 2 — Objects, member access, prototypes, `new`, and `Error`

Object literals; property get and set; prototype chains with `Object.getPrototypeOf` /
`setPrototypeOf`; `new` and constructor calls; `typeof`, `in`, `instanceof`; strict equality
extended to reference identity; and enough of `Error` to construct one with a `message`, a string
`stack`, an options bag carrying `cause`, and a `captureStackTrace` static.

**Forced relative to everything above it** — arrays are objects with a prototype, `Symbol` is an
object, classes are prototype machinery. The choice of *`Error` specifically* at this rung is
**convenient, not forced**: `Error` is here because g15 is the cheapest probe in the corpus and
this is the earliest point at which anything at all can pass. Nothing below step 2 needs `Error`;
something to pass does.

**Predicted unlocks: g15. Running total 1/20.**

g15 is a good first target for a second reason. `e.cause === inner` is a reference-identity
comparison, which is the cheapest available test that step 1's heap gave values a real identity
rather than a boxed copy.

### Step 3 — Arrays, `Symbol`, computed access, and the first real builtin surface

Array literals with indexed elements and `length`; a real `Array.prototype` carrying `join`,
`push`, `map` and `values`; `Symbol` with the well-known symbols; computed member access (`o[k]`);
globals `Array`, `Object`, `JSON`, `String` as ordinary bindings whose statics resolve through a
variable rather than by name at the call site; `Object.keys` excluding symbol keys.

**Forced before step 6.** `Symbol.iterator` is the key the iterator protocol dispatches on; the
protocol cannot be written before symbols and computed access exist. `Array.prototype` carrying a
real `@@iterator` is the same requirement seen from the other side.

The "through a variable" wording is not pedantry — it is what g10 exists to catch, and
`conformance/README.md` records Effect doing `const Array = globalThis.Array` before calling
`Array.isArray`. A compiler that special-cases `Array.isArray` at the call site passes nothing
useful.

**Predicted unlocks: g02, g06, g06b, g10. Running total 5/20.**

One caveat inside this step: `[1,2].map(fn)` is a builtin calling *back into* JavaScript. If `map`
is written in JS per design.md §5 that is free. If it is written in Rust, the callback boundary is
a mechanism that has to be designed, and then designed again when `map` moves to JS. See §6.

### Step 4 — Spread, rest, and destructuring, with symbol keys intact

Object spread `{...src}`; object rest `const {a, ...rest} = src`; array and parameter
destructuring; computed keys in object literals; and the rule that all of these carry
symbol-keyed own properties while `Object.keys` still excludes them.

**Convenient, not forced.** Nothing below step 4 depends on it, and it could sit anywhere after
step 3. It is placed here because it is the cheapest remaining `+1` and because it is a hard
prerequisite for every Effect program: `conformance/README.md` records that Effect builds every
prototype by spreading one proto into another, and `[Symbol.iterator]` on those protos is what
makes an Effect yieldable at all. Doing it early means it is load-bearing and tested long before
Effect arrives to depend on it.

**Predicted unlocks: g01. Running total 6/20.**

### Step 5 — The rest of control flow, and exceptions

`for`, `do`-`while`, `break`, `continue` (including labelled forms), `switch`, `throw`, and
`try`/`catch`/`finally` — with `finally` running on all four abrupt exit paths: normal completion,
`return`, a propagating throw, and `break`/`continue` out of a loop.

**Forced before steps 6 and 9.** IteratorClose is an abrupt-completion mechanism: `break` out of a
`for..of` is exactly the path that has to call the iterator's `return()`, and it is one of the
four paths g08 proves can be silently dropped. Generator `.return()` in step 9 has the same
dependency — it must unwind the suspended body so its `finally` runs.

`conformance/README.md` singles this out as the most dangerous class of bug in the whole corpus,
because it is silent rather than loud: "every JS-level resource cleanup and every generator
`.return()` interruption path just quietly does not happen." Build it before the things that
depend on it, not after.

**Predicted unlocks: g08. Running total 7/20.**

### Step 6 — The iterator protocol

`GetMethod(obj, @@iterator)`, then drive `.next()` / `done` / `value`, shared by `for..of`,
iterable spread, `Array.from`, and (later) `yield*`. Not a type-tag switch over built-in types.
`@@iterator` must be a real, observable, identity-stable property on the built-in prototypes,
because libraries feature-detect with `Symbol.iterator in v`. IteratorClose runs `return()` on
early exit.

**Forced before steps 7 and 9** — Set, Map and generator objects are all required to be iterable
through this same mechanism, and `yield*` is the delegation form of it.

Two constraints from the corpus are worth carrying into the implementation. First, the protocol
has to be generic from the start; retrofitting genericity onto a type-tag switch is what the
source project had to undo. Second, presence is not the same as behaviour: g06b asserts
`[][Symbol.iterator] === [].values`, so the property must be the *same function object*, not a
second one that behaves identically.

The corpus also records two things Effect did **not** need, so they should not be built here:
array-destructuring of a generic iterable, and iterator close on `return`/`throw`/labelled jump.
The first turns a loud TypeError into a hang on an infinite iterable, which is a bad trade.

**Predicted unlocks: g04, g18. Running total 9/20.**

### Step 7 — Set and Map

`Set` and `Map` with insertion-ordered iteration, `@@iterator` as a real property, and iteration
that tolerates deletion of entries during the walk.

**Forced before step 9 only for g17**; otherwise its position relative to step 8 is a free choice.
It is placed before classes because `class M extends Map` in step 8 needs `Map` to exist, and
because the mutation-during-iteration requirement is a data-structure decision best made before
anything subclasses the structure.

That requirement is why this step is not trivial. From `conformance/README.md`: `race` and
`timeout` interrupt their children by deleting each one from a Set as they iterate it, so this
single behaviour gates *all* of Effect's interruption. A dense backing array compacted under a
live cursor — the natural first implementation — is precisely the bug.

**Predicted unlocks: g13. Running total 10/20.**

### Step 8 — Classes and derived constructors

Class declarations and expressions; instance and static fields whose initialisers capture the
enclosing scope; methods; `extends`; `super()`; real derived-constructor semantics where `this`
binds to the result of `super()`; statics and static inheritance through a prototype set on a
*function*; private names with correct brand checks in both directions; and subclassing of exotic
builtins (`Array`, `Map`, `Error`).

**Forced before step 9's g05** (which is two class declarations plus a generator), and forced
before every Effect program from 10-tagged-error onward.

The derived-constructor rule is the load-bearing one and it pays for itself: the source project
found that once derived constructors were correct, its special-cased `Error` patch could be
deleted outright. Do not model errors as a distinct primitive type tag; make them ordinary objects
with a real prototype chain and the `Error` cases fall out.

**Predicted unlocks: g03, g09, g11, g12, g19. Running total 15/20.**

This is the largest single delta on the ladder, which is an argument against deferring it. It is
also the prediction most likely to be wrong, simply because it is the largest — if classes land
and only three probes go green, the two stragglers are naming a real missing dependency.

### Step 9 — Generators

`function*`, `yield`, suspended frames, generator objects carrying `@@iterator`, `yield*`
delegation with values passed back in through `.next(v)`, and `.return(v)` unwinding the suspended
body so its `finally` runs before completing with `{ value: v, done: true }`.

**Forced after steps 5 and 6** (finally unwinding; the protocol it delegates through). Its
position relative to step 8 is **convenient**: swapping 8 and 9 moves g05 one rung earlier and
g14/g16/g17 one rung later, a wash on total count. Classes go first because they unlock more
probes sooner and because generator *methods* in classes are then a natural increment rather than
a merge of two half-finished features.

Note that `yield*` lowered to a `for..of` inherits every `for..of` limitation, which is why g05
exists separately from g04 — implementing delegation on top of step 6 is right, but it needs its
own test.

**Predicted unlocks: g05, g14, g16, g17. Running total 19/20.**

### Step 10 — Timers, microtasks, and the event loop

`setTimeout` with forwarded arguments, `clearTimeout`, `setInterval` cleared from inside its own
callback, `queueMicrotask`, and correct ordering between microtasks and timers.

**Forced last among the ten** only in the weak sense that everything it schedules is a function
call. In practice it is independent of steps 4 through 9 and could be built in parallel by someone
else — it is the one obviously parallelisable rung.

design.md §11 wants these driven by a mock event driver's virtual time rather than by real
sleeping, and the manifest already tags the right tests (`timers`, `event-loop`) for it. Build the
driver with the timers, not after: `conformance/README.md` records a timing-sensitive test that
"produced no output" on a loaded machine and had simply not finished.

**Predicted unlocks: g07. Running total 20/20.**

### The ladder at a glance

| # | step | position | unlocks | running |
|---:|---|---|---|---:|
| 1 | heap, functions, calls, closures | forced | — | 0/20 |
| 2 | objects, members, prototypes, `new`, `Error` | forced (Error: convenient) | g15 | 1/20 |
| 3 | arrays, `Symbol`, computed access, builtin statics | forced before 6 | g02 g06 g06b g10 | 5/20 |
| 4 | spread, rest, destructuring with symbol keys | convenient | g01 | 6/20 |
| 5 | `for`/`break`/`continue`/`throw`/`try`/`finally` | forced before 6 and 9 | g08 | 7/20 |
| 6 | iterator protocol, `for..of`, `Array.from`, close | forced before 7 and 9 | g04 g18 | 9/20 |
| 7 | Set and Map, mutation-safe iteration | convenient (before 8) | g13 | 10/20 |
| 8 | classes, derived constructors, private names | forced before 9 and Effect | g03 g09 g11 g12 g19 | 15/20 |
| 9 | generators, `yield*`, `.return()` | forced after 5 and 6 | g05 g14 g16 g17 | 19/20 |
| 10 | timers, microtasks, event loop | independent | g07 | 20/20 |

Where a step's position is marked convenient, reordering costs nothing — but it should be a
decision rather than an accident, and the prediction column has to be re-derived if it changes.

---

## 5. Where the 12 Effect programs sit

Not on this ladder. Above it.

Every one of the twelve imports `effect` and evaluates the entire library at module load. There is
no partial credit: 01-core's three lines and 12-service-layer's ten both require the same library
initialisation to complete without throwing. So the distance is not "01-core is easy and 12 is
hard" — it is one large shared floor with modest increments on top.

Today, `kiln --one-shot run conformance/effect/src/01-core.ts` produces exactly one interesting
diagnostic before the rest:

    module dependency `effect` is not supported by this execution backend

That refusal comes from `kiln-ir`, which rejects every entry in `module.requested_modules`. There
is no module resolver in the workspace at the time of writing.

### The shared floor, measured

Steps 1 through 10 above are necessary and nowhere near sufficient. On top of the full ladder,
every Effect program additionally needs:

- **Module loading and module semantics.** Real ESM resolution for the `.ts` sources. For the
  bundles — which esbuild has already stripped of every `import` and `export` — module *semantics*
  still matter, because Effect picks its scheduler with
  `"setImmediate" in globalThis ? … : setTimeout` wrapped around its own top-level binding. If a
  top-level `var` in a module leaks onto the global object, the wrong branch is taken and fork
  scheduling breaks. That is what 09-fiber is testing.
- **Promises with a real microtask queue.** `runPromise(...).then(...)` is how ten of the twelve
  produce their output.
- **A large builtin surface**, quantified immediately below.

The builtin surface is measurable rather than a matter of opinion. Counting identifier
occurrences in the committed 209 KB bundle (`12-service-layer`, 6,187 lines):

| global | uses | global | uses |
|---|---:|---|---:|
| `Object` | 69 | `WeakMap` | 8 |
| `Array` | 32 | `Math` | 8 |
| `Error` | 28 | `JSON` | 6 |
| `Map` | 24 | `WeakSet` | 5 |
| `globalThis` | 23 | `Reflect` | 4 |
| `Symbol` | 22 | `setTimeout` / `clearTimeout` | 4 |
| `Number` | 20 | `Uint8Array` | 3 |
| `Set` | 18 | `RegExp` | 3 |
| `BigInt` | 18 | `performance` | 3 |
| `String` | 11 | `ArrayBuffer` | 3 |
| `setImmediate` | 11 | `Promise` | 2 |
| `Date` | 10 | `Proxy`, `process`, `Function`, `RangeError` | 1 each |

The statics actually named are specific enough to serve as a checklist: `Object.assign`,
`create`, `defineProperty`, `entries`, `getOwnPropertyDescriptor`, `getPrototypeOf`, `hasOwn`,
`is`, `isExtensible`, `keys`, `prototype`, `setPrototypeOf`, `values`; `Array.from`, `isArray`,
`prototype`; `Symbol.for`, `iterator`, `dispose`, `asyncDispose`; `Number.isFinite`, `isInteger`,
`isNaN`, `MAX_SAFE_INTEGER`, `MIN_SAFE_INTEGER`, `POSITIVE_INFINITY`; `Math.ceil`, `floor`, `max`,
`min`, `random`, `round`; `JSON.stringify`; `Reflect.ownKeys`; `Error.stackTraceLimit`;
`Promise.resolve`; `Date.now`. Beyond that the bundle contains 163 distinct `.method(` names,
which over-counts (Effect's own methods are in there) but indicates the scale of the prototype
surface.

That is not a small extension of step 3. It is roughly `Object`, `Array`, `Symbol`, `Number`,
`Math`, `String`, `Error`, `Map`, `Set`, `WeakMap`, `WeakSet`, `Promise`, `Date`, `JSON`,
`Reflect` and `BigInt` at real fidelity — plus `Proxy` and typed arrays on paths that may or may
not execute.

### Which one could pass first

**01-core**, and it is the right one to aim at.

The reasoning: 02-gen has the smaller source and the smaller API surface (`gen`, `succeed`,
`runSync`), and it avoids promises entirely, which makes it tempting. But `Effect.gen` needs the
`yield*` delegation protocol over a user `@@iterator` reached through a spread-preserved symbol
key — three of the hardest items on the ladder, composed. 01-core's marginal requirement over the
shared floor is a resolved promise and a microtask queue, which is a smaller and far
better-understood engine feature than generators.

`conformance/manifest.toml` supports this indirectly: 02-gen's own note calls it "the smallest
program that needs both of the two defects that originally blocked Effect entirely." Smallest is
not the same as first.

I am guessing here, and the guess is falsifiable in a specific way: it assumes Effect's module
initialisation does not itself *drive* a generator or reach the timer path. If library init turns
out to call `Effect.gen` internally, 01-core and 02-gen collapse into the same target and the
ordering claim is void.

The rough shape of the remaining ten, read from the manifest and the sources:

| program | marginal requirement above 01-core |
|---|---|
| 02-gen | generators, `yield*` over a user `@@iterator` |
| 03-ref | nothing structural — mutable state through `runPromise` |
| 04-all-foreach | `Array.isArray` off a variable, `Symbol.iterator in v` (g10 + g06b) |
| 05-concurrency | a scheduler preserving *input* order regardless of completion order |
| 06-sleep | a real event loop, not just a microtask queue |
| 07-interruption | Set mutation during iteration + generator `.return()` unwinding (g13 + g14) |
| 08-scope | nested `Effect.gen` closing over a mutable local |
| 09-fiber | module semantics: top-level `var` must not reach `globalThis` |
| 10-tagged-error | class fields capturing a parameter, inside a class extending `Error` (g03 + g09) |
| 11-stream | the widest library surface of the twelve (377 KB bundle), ending in `Array.from` |
| 12-service-layer | `Object.setPrototypeOf` on a function + static-side `@@iterator` (g11), plus everything above |

12-service-layer is the source project's definition of done, and it is the right definition to
inherit: dependency injection, generators, concurrency and timers in one program.

The honest summary: **guard 20/20 is a precondition for the Effect corpus, not a predictor of it.**
Finishing the ladder gets you to the point where the remaining work is a builtin corpus and a
module loader, and those are large.

---

## 6. Builtins: when they stop being premature

**Today there are none.** `HostOp` has exactly one variant, `WriteStdout = 1`, at
`HOST_ABI_VERSION = 1`. There is no `builtins/` directory anywhere in the tree. The only
acknowledgement that builtins will exist is in `kiln-test-selector`, which already understands
`builtin-<module>` feature tags and a `builtins/` path component — the invalidation plumbing is
ready for a corpus that does not exist.

design.md §5 wants as much runtime behaviour as possible written in JS/TS. ADR 0005 accepts that
and calls it "the most valuable idea in the design document" and also "the highest-risk", for the
structural reason that it makes the compiler depend on its own output.

### When they stop being premature: the end of step 2

A builtin written in JS needs, minimally, functions with closures and `this` (step 1), and objects
with prototypes and property definition (step 2). Before that point there is nothing to hang
`Array.prototype.join` on and no `this` for it to receive, so "writing builtins" means writing them
in Rust and rewriting them later — precisely the cost §5 exists to avoid.

The right first move at the end of step 2 is **one tiny module, not a corpus**. ADR 0005's central
mitigation is that built-in compilation is a normal query in the database rather than a side
channel, and the builtins are the first realistic cross-referencing input the query engine will
ever meet. Proving that path end to end is much cheaper with one module than with thirty, and if
the design is wrong, one module is what you throw away.

### When they become mandatory: step 3

`[1,2].map(fn)` is a builtin calling back into JavaScript. Written in JS it is ordinary code;
written in Rust it is a callback boundary that has to be designed and then designed again when
`map` moves to JS. Step 3 is therefore the last comfortable moment to decide — and the decision is
already made in §5, so this is just the point at which it has to be acted on.

### The one thing that has to be settled first

`HostValue` is scalar-only: the same five variants as `Value`, with no reference case. A builtin
written in JS still has to hand objects across the HostOps seam for anything I/O-shaped. The ABI's
object representation is therefore the gating design decision, and ADR 0005 has already recorded
the consequence: "The HostOps ABI has to be designed earlier than §14's ordering suggests." Step 2
is that earlier point.

### On porffor

Porffor's builtins — around 15,000 lines, per the framing of this task; I did not count them, as
porffor is not in this repository — are a reference to **consult**, not code to port.

They are written against a linear-memory model and `Porffor.*` intrinsics that kiln does not have
and is not going to have: kiln's values are a Rust enum and its host seam is a typed `HostOps`
trait. A line-by-line port would mean translating from a memory model kiln has already rejected.

What is worth taking is the behavioural content: which statics exist, what argument validation each
performs, which edge cases turned out to matter. That is the expensive-to-rediscover part. Kiln's
own version of the same information already exists in a more usable form — the statics list in §5
above is measured from a real program, and `conformance/manifest.toml`'s `catches` fields record
twenty specific behaviours that a real library depended on.

What is worth taking from porffor's *shape* is its warnings, and ADR 0005 has already extracted
them: the two-pass precompile, the one-shot comptime flags, and the lazily materialised prototypes.
That third one has a direct consequence for how the builtin corpus is tested — the ADR requires
conformance cases that exercise a builtin with nothing else in the program, and
`conformance/guard/g06-array-proto-iterator.js` is exactly that shape, deliberately, with the
reason recorded in the manifest. A one-line probe that names only its subject is not a stylistic
preference; it is the only kind of probe that can catch a laziness bug.

---

## 7. What is measured, what is inferred, and what is guessed

**Measured** (re-runnable with the commands at the top): everything in §1 and §2; the per-probe
blocker sets in §3; the diagnostic and identifier counts in §5. `examples/hello.ts` printing
`kiln ready 3`. The 727 diagnostics and zero parse errors on the 209 KB bundle. The `-0` divergence
against node v25.9.0.

**Inferred from reading the source** (`crates/kiln-ir/src/lib.rs`,
`crates/kiln-interpreter/src/lib.rs`, `crates/kiln-hir/src/lib.rs`, `crates/kiln-host-abi`): the
absence of a heap, of call frames, and of any object representation; the `console.log` special case
and its `resolve("console")` guard; the shape of the `Value` and `HostValue` enums.

**Guessed, and flagged as such:**

- The predicted unlocks per step are predictions. They are derived from each probe's measured
  blocker set, but a step can land and still miss a probe on a dependency nobody listed. That is
  the point of writing them down.
- Step 8 unlocking five probes at once is the weakest prediction on the ladder, simply because it
  is the largest.
- 01-core being the first Effect program that could pass rests on the assumption that Effect's
  module initialisation does not itself drive a generator or reach the timer path. Not verified.
- The Effect "marginal requirement" table is read off the manifest and the twelve source files,
  not off a run. Nothing in `conformance/` has ever run under kiln.
