# `fuzz/` — differential fuzzing for kiln

kiln's central architectural claim is that **two independently written engines produce
byte-identical output from the same bytecode**: the Rust VM in `kiln-interpreter`, and the C that
`kiln-backend-c` emits and a platform compiler turns into a native binary. `scripts/native.sh`
checks that claim over a hand-written corpus, and `conformance/README.md` says exactly what a
corpus costs: *"a real program composes features in combinations nobody wrote a test for"*. This
crate writes those programs.

It found three on the day it was built — one root cause, reached three ways, in 1,500 generated
programs. It then grew a **third reference**, because two engines that share a frontend, a lowering
and a set of builtins agree about their shared mistakes: adding `node` and generating values that
answer back during conversion found five more, three of which the two-engine comparison reports as
a *pass*. See **Findings** and **Findings, node as the reference** below.

## Contents

| target | what it compares | measured cost per case (one development machine) |
|---|---|---|
| `differential_native` | interpreter stdout vs native stdout, byte for byte | **446 ms** (clang) / **819 ms** (zig) |
| `node_reference` | the batched node reference against a real `node` process, and five pinned findings | 48 ms |
| `structured_ir` | `kiln_backend_c::validate` against the interpreter's own idea of well-formed | 232 µs |
| `memo_oracle` | a memoized query engine against a from-scratch one | 23 µs |
| `generator_health` | the generators themselves, against being empty and against missing a shape | 7 µs |

and two soak modes, which are where the work actually happens:

| mode | what it compares | measured cost per case |
|---|---|---|
| `soak --node-screen` | node vs the interpreter, escalating only disagreements to a native build | **1.22 ms** |
| `soak --node` | node vs the interpreter vs the native backend, every case | **569 ms** |

Everything is standalone: `fuzz` is excluded from the root workspace (`exclude = ["fuzz"]`,
ADR 0004) and carries its own lockfile, so bolero, libFuzzer and a C++ runtime never enter the
compiler developer's `cargo check --workspace` loop.

## Commands

The differential targets below primarily look for wrong answers. The native runtime and
shared-graph arena also contain unsafe code; boundary campaigns in [`security/`](security/README.md)
support sanitizer instrumentation and exercise malformed inputs.

```sh
cd fuzz

# Everything, at sizes that belong in a test run. ~15 s, of which 7 s is eight native cases.
cargo test

# One target.
cargo test --test structured_ir
cargo test --test memo_oracle
cargo test --test differential_native -- --nocapture

# More of a target. Every target reads the same variable.
KILN_FUZZ_ITERATIONS=400000 cargo test --release --test structured_ir     # 93 s
KILN_FUZZ_ITERATIONS=600000 cargo test --release --test memo_oracle       # 14 s
KILN_FUZZ_ITERATIONS=500    cargo test --release --test differential_native -- --nocapture
```

### Coverage-guided (`cargo bolero`)

```sh
cargo install cargo-bolero            # once
cd fuzz

cargo bolero test the_validators_agree_about_well_formedness \
    --engine libfuzzer --sanitizer NONE -T 60sec
cargo bolero test memoized_agrees_with_a_fresh_engine_at_every_step \
    --engine libfuzzer --sanitizer NONE -T 60sec
```

Observed: 201,160 runs in 21 s for the memo oracle (coverage 1061, corpus 420), 18,834 runs in
21 s for `structured_ir` (coverage 3190, corpus 894). Instrumentation costs roughly 5× against
the plain RNG engine and buys a corpus that keeps whatever reached new code.

`--sanitizer NONE` is used throughout: cargo-bolero's default `--sanitizer address` needs
`-Zsanitizer` and an extra instrumented build, and is optional for the semantic campaigns below. Use ASan for the boundary campaigns. The repository
pins a nightly toolchain (`rust-toolchain.toml`), which `fuzz/` inherits, but the flag keeps the
run independent of sanitizer support.

Two things had to change in this crate to make `cargo bolero` build at all, and both are worth
knowing before adding a file here:

- **Every test target must call `bolero::check!`.** cargo-bolero instruments *all* targets in the
  package with sanitizer coverage, but only targets that link the `bolero` rlib get libFuzzer's
  runtime, which is what defines `__sanitizer_cov_*`. A test file with no bolero target in it
  fails to link. That is why the two pinned findings live inside `tests/structured_ir.rs` rather
  than in a file of their own.
- **`src/lib.rs` has `use bolero as _;`.** The two binaries (`soak`, `differ`) do not mention
  bolero, so without this the rlib is not in their link graph and they fail the same way.

### Soaking, and reproducing

```sh
cd fuzz

# node as a third reference. 20,000 cases in 24 s, and the run that produced findings 4 to 8.
cargo run --release --bin soak -- --node-screen --cases 20000 --seed 2026

# All three references on every case. Thorough, and 466x the price.
KILN_FUZZ_CC=/usr/bin/clang cargo run --release --bin soak -- --node --cases 200 --seed 7

# One whole generated program per distinct finding, written out as a starting point for reducing.
cargo run --release --bin soak -- --node-screen --cases 20000 --findings /tmp/leads

# The long-running mode for the native target. Prints the outcome distribution, the reason
# distribution, and the measured cost breakdown; exits non-zero on any defect. Eleven minutes,
# and the run that produced finding 1.
cargo run --release --bin soak -- --cases 1500 --seed 101

# Same generator, no C toolchain: ~15,000 cases/second. This is how the generator was tuned.
cargo run --release --bin soak -- --cases 5000 --interpret-only
cargo run --release --bin soak -- --cases 5000 --lower-only

# One file through all three, which is what you use on a reported divergence. node and the C
# toolchain are each optional and each say so when missing.
cargo run --release --bin differ -- findings/symbol-to-primitive-is-ignored.js

# Which compiler. Discovery finds zig first here and clang is twice as fast; see the cost table.
KILN_FUZZ_CC=/usr/bin/clang cargo run --release --bin soak -- --cases 1500
```

---

## Findings

### 1. A silent wrong answer: ToNumber does not run ToPrimitive on the builtin-call path

**This is a divergence of the kind the project ranks worst — both engines ran to completion, both
exited 0, neither said anything, and they printed different bytes.**

```js
console.log(Math.abs([0]));
```

```
node:        0
interpreter: 0
native:      NaN
```

Reduced from a 25-line generated program (`soak --cases 1500 --seed 101`, case 107) to one line.
`findings/tonumber-on-an-array.js` maps the edges:

| expression | node & interpreter | native |
|---|---|---|
| `Math.abs([0])`, `Math.abs([2])`, `Math.abs([])` | `0`, `2`, `0` | `NaN` |
| `Math.floor([4])`, `Math.trunc([5])` | `4`, `5` | `NaN` |
| `Number([6])`, `Number(["2"])` | `6`, `2` | `NaN` |
| `+[7]`, `[8] * 1`, `[1] - 0` | `7`, `8`, `1` | correct |
| `Math.abs("3")`, `Number(true)`, `Number(null)` | `3`, `1`, `0` | correct |

So the operator paths convert an array correctly and the builtin-call paths do not; primitives
convert correctly everywhere. It is ToPrimitive that the builtin path is missing, not ToNumber.
The interpreter agrees with node in every row, so the native backend is the wrong one.

The same seed found two more in the same run — `Math.trunc([])` at case 671 and `Math.abs([])` at
case 1425, each `0` against `NaN` — the same root cause reached through different builtins.

### 2. `validate` licenses C for bytecode the interpreter aborts on

Two findings of one shape, from `structured_ir`. `kiln_backend_c::emit` runs a static validator and
then generates C that *trusts* it: no bounds check on a constant index, no tag check before a value
is used, because `validate` promised there was no need. Where the interpreter's own idea of
well-formed is stricter, the asymmetry is not in the interpreter's favour — it aborts the process,
and the generated C has no opinion at all.

Both are two instructions long, both are accepted by `emit`, and both are pinned as tests in
`tests/structured_ir.rs` that fail when the finding is fixed:

- **A cell read as a local.** `NewCell(0); LoadLocal(0)` — the slot holds a cell and `LoadLocal`
  reads the slot rather than through it, so the marker reaches an expression and
  `unreachable!("a cell is never used as a value")` kills the process.
- **An array hole used as a value.** `PushHole; Call { argc: 0 }` — the marker for a missing
  element of an array literal, called, reaches native dispatch and
  `unreachable!("an array hole is never a value")`.

Neither is reachable from source today: lowering decides per binding whether a slot is a cell, and
only ever emits `PushHole` between the elements of a `NewArray`. That is why they are recorded as
validator gaps rather than as wrong answers. What they show is that `validate` is not currently a
sufficient precondition for the unchecked C it licenses, and the generator suppresses both shapes
(documented at each suppression) so they do not mask everything else.

### 3. What the native backend refuses, in proportion

From those 1,500 cases, the native runtime's `KILN_C99_CONTROL_ERROR` (`-2`) is the whole backlog:
1,250 of the 1,357 non-matching outcomes, against 107 ordinary uncaught exceptions. It is what the
emitted code returns when it meets something outside its implemented surface — a property read
whose key is not in its method table, for one — and the launcher prints why and exits 1.

Of the 938 cases the interpreter ran to completion, **140 matched byte for byte and 795 hit a
control error**: the native surface currently covers about one in seven of the programs this
generator writes. That is a work queue and not a defect — it names itself, and nobody is misled by
it — but it is also the number to watch, because every point it rises is another program the
differential comparison can actually make.

---

## node, and why two engines agreeing is not evidence

`differential_native` compares the interpreter against the native backend. They consume the same
bytecode, out of the same lowering, behind the same frontend, and they inherit every decision
either of those made. **When both are wrong in the same way the comparison sees agreement and
reports a pass**, and that is not a hypothesis: `NOTES.md` records it twice in the project's own
words — *"Both engines were wrong in the same way, which is why no corpus caught it: they
agreed."* — once for frozen arrays and once for `delete`.

So `src/node.rs` adds an implementation that shares nothing with either: `node`, off `PATH`. The
convention is `scripts/differential.sh`'s and is not reinvented — the reference answer is
**observed by running the program**, never transcribed, and node runs under `TZ=UTC` because kiln's
realm is always UTC (ADR 0015) and a zone read from the environment is host state.

### The outcomes, kept apart on purpose

A fuzzer that cries wolf gets switched off, and the cheapest way to cry wolf is to put a refusal
and a wrong answer in the same bucket. `src/triage.rs` does not:

    GENERATOR-BUG          node refused the source. `js.rs` emitted something that is not
                           JavaScript. Nothing has been learned about kiln, and the fix is in this
                           crate. `tests/node_reference.rs` asserts this never happens.
    not-comparable         lowering refused, or the interpreter's screening budget stopped it.
    refused                a kiln engine stopped inside its implemented surface, having printed a
                           prefix of what node printed. The backlog: the growing native surface,
                           and ADR 0017's unresolved identifiers.
    agrees                 everything that ran printed the same bytes.

    INTERPRETER-vs-NODE    the interpreter disagreed with node and the native backend did not.
    NATIVE-vs-NODE         the native backend disagreed with node and the interpreter did not.
                           Also an engine/engine divergence — reported this way because node
                           naming *which* engine is wrong is more than two engines could say.
    ENGINES-DIVERGE        the two engines disagreed with each other and node matched neither.
    BOTH-ENGINES-vs-NODE   **both engines printed the same bytes and node printed different ones.**
                           The class this exists for, and a silent pass under the two-engine
                           comparison.

The rule underneath all of them is `engines.rs`'s, extended to a third party: **an engine that
stops is allowed to have printed less, and an engine that finishes is not allowed to have printed
anything different.** One corollary is worth stating because it caught the largest finding below:
a kiln engine that runs to completion where *node threw* is a wrong answer too. It produced a value
node says the program does not have.

Two things the classification refuses to do, both for the same reason — a headline it cannot
support is worse than no headline:

- A native **refusal** is never counted as the native backend agreeing with the interpreter, so
  `BOTH-ENGINES-vs-NODE` is never claimed on an answer nobody obtained.
- `engines::observe` exists because `engines::compare` collapses a match into "the same bytes as
  the interpreter" and does not keep them. Claiming both engines printed something requires having
  the bytes.

### Cost, measured

| stage | measured |
|---|---|
| one `node program.js` process | 47.9 ms |
| one program through the batched reference | **1.13 ms** |
| `--node-screen`, whole loop, 20,000 cases in 24.34 s | **1.22 ms** |
| `--node`, whole loop, 192 cases in 109.16 s | **568.6 ms** |

The 42× between the first two rows is `src/node_batch.js`: sixty-four generated programs in one
node process, each in its own `vm` context. Nearly all of a `node program.js` is process startup,
and at 48 ms a case node would *be* the run — a 20,000-case screen would take sixteen minutes
instead of twenty-four seconds.

**What would make that worthless is formatting that is not node's.** `console.log` does not print
values, it inspects them — `[ 1, 2 ]`, `{ p0: 'x' }`, `<1 empty item>` — and a driver whose
formatting drifted by one space would report a divergence on every program that logged an array. So
the driver formats nothing: it hands a real `node:console` a `Writable` instead of fd 1. That is
pinned, not assumed —
`node_reference::the_batched_reference_prints_exactly_what_a_real_node_process_prints` runs nine
hand-written formatting cases and twenty-four generated programs through both paths and requires
byte equality.

**The escalation is the point of `--node-screen`.** node plus the interpreter is 1.22 ms; a native
build is 446 ms. Cases where node and the interpreter already agree do not get a C compilation,
because there is nothing there for the native side to settle that a plain `--cases N` run would not
settle more cheaply. That is 466× more programs per hour, against giving up the engine/engine
comparison on cases that looked fine — which is exactly the comparison the original mode runs.

### What is excluded, and why

The four properties `js.rs` guarantees — termination, bounded work, determinism, resolvability —
were already what made a two-engine comparison meaningful, and every one of them is load-bearing
again here for a different reason: node has a real clock, real entropy and a real heap.

- **No `Date`, no `Date.now`, no `new Date()`, no timers.** kiln's clock is virtual and node's is
  not (ADR 0012, ADR 0015). `scripts/differential.sh` documents the same trap.
- **No `Math.random`.**
- **No `Map`, no `Set`, no `for..in`.** Iteration order over a `Map` keyed on objects is
  insertion order in both, but nothing in this grammar can produce two distinct objects with a
  stable relative order to key it on, so the generator does not go there at all.
- **No unbounded loops and no user-written loop condition.** The one loop form is
  `for (let i = 0; i < K; i++)` with `K` a literal in `0..=3` and an induction variable nothing can
  assign to. Nesting is capped at two, so the innermost body runs at most nine times.
- **No recursion.** Calls are erased inside function bodies, so the call graph is one level deep.
- **`node --max-old-space-size=512`**, a 10 s per-program timeout and a 1 MB output ceiling in the
  driver. All three are third fences behind the generator bounds and the interpreter screen, and
  they are here because a fuzzing loop that can be made to allocate without bound eats the machine.
  It is much cheaper to lose a case than a machine.

The ordering is a safety property and not a preference: **lower, interpret, then node.** The
interpreter's 50,000-step screen is what bounds everything downstream, so node is only ever handed
a program the cheap engine already finished.

### False positives, and what was done about each

| what it was | what it turned out to be |
|---|---|
| 517 of 2,000 cases reported as `INTERPRETER-vs-NODE`, most with node printing nothing at all | **Not a false positive.** One real bug — finding 4 below — reached 828 different ways. |
| The same bug arriving as **155 distinct findings** | A real reporting defect. When node stops and kiln does not, the bytes the two "disagree on" are whatever the generated program happened to print next, so grouping on the differing line split one root cause across a hundred entries. `Verdict::signature` now keys those on node's *reason for stopping*: 155 entries became 1. |
| Signatures reading `node=<end> kiln=<end>`, which name nothing | Every line matched and the outputs still differed — a trailing newline, or a `\r`, which `str::lines` folds away. The signature now says so and gives both byte counts. |
| A lone surrogate reaching stdout | Genuinely lossy at the process boundary — node writes U+FFFD — but **both sides are lossy identically**, and the underlying bug (finding 6) is visible in `.length` without going near the boundary. |

Two things are deliberately *not* suppressed even though each fires often. The
`Cannot convert object to primitive value` class is 21% of all generated programs, and the
`console.log` formatting gaps fire on any program that logs a wide object — both are real, both are
one root cause each, and the deduplicated report shows each as one line with a count. Suppressing a
real finding because it is common is how the frozen-array bug survived.

---

## Findings, node as the reference

Five root causes from `soak --node-screen --cases 20000 --seed 2026` (24 seconds) and
`--cases 4000 --seed 101`. Every one is reduced to a one-liner in `findings/`, and every reduction
was re-checked against node before it was written down. **Three of the five are wrong in both
engines**, which is to say invisible to the comparison this crate started with.

### 4. ToPrimitive invents a value where the specification says the program has none

`findings/no-primitive-available-does-not-throw-minimal.js` — **BOTH-ENGINES-vs-NODE**, and the
single most common finding in the corpus: **4,160 of 20,000 generated programs**, 21%.

```js
console.log({ valueOf: () => ({}), toString: () => ({}) } + 1);
```

```
node:                   TypeError: Cannot convert object to primitive value   (exit 1, no stdout)
interpreter and native: [object Object]1
```

When neither method returns a primitive, step 6 of OrdinaryToPrimitive raises. kiln does not — so a
program that must stop keeps running, and everything after that point is output node never produces
at all. Both engines agree, so `differential_native` calls it a match.

### 5. `Symbol.toPrimitive` is not consulted

`findings/symbol-to-primitive-is-ignored.js` — **INTERPRETER-vs-NODE**: the native backend is
right and the interpreter is wrong.

```js
const o = { [Symbol.toPrimitive]: (hint) => (hint === "string" ? "S" : 5) };
```

| expression | node & native | interpreter |
|---|---|---|
| `Number(o)` | `5` | `NaN` |
| `String(o)` | `S` | `[object Object]` |
| `o + 1` | `6` | `[object Object]1` |
| `` `t${o}` `` | `tS` | `t[object Object]` |
| `o < 1` | `false` | `false` |

The last row is why this needed finding rather than reasoning about: relational comparison happens
to agree, so a probe that only tested `<` reports a pass.

### 6. A lone surrogate escape is read as five code units

`findings/lone-surrogate-escape-minimal.js` — **BOTH-ENGINES-vs-NODE**, and necessarily so: this
is the frontend reading a string literal, and both engines are handed the same wrong string. There
is no arrangement of kiln components that could have caught it.

```js
console.log("\ud800".length);   // node 1, kiln 5
console.log("a\ud800b".length); // node 3, kiln 7
```

kiln reads `"\ud800"` as a replacement character followed by the literal text `d800`. The string is
already in `js.rs`'s `STRINGS` pool, chosen for ADR 0022's reasons; it took a third reference to
notice that kiln does not read it.

### 7. `Object.keys` of a string is empty

`findings/object-keys-of-a-string.js` — **INTERPRETER-vs-NODE**; the native backend refuses this
program with `KILN_C99_CONTROL_ERROR`, so it has no opinion.

```js
Object.keys("abc")            // node [ '0', '1', '2' ]   interpreter []
Object.keys([7, 8])           // node [ '0', '1' ]        interpreter [ '0', '1' ]   (correct)
Math.abs(Object.keys("t0"))   // node NaN                 interpreter 0
```

Arrays are right, which is what makes this specific. The third line is how it was found: an empty
array converts to `0` and a two-element one does not convert at all, so the wrong answer travels.

### 8. Three gaps in `console.log`'s inspection

`findings/console-inspect-gaps.js` — **BOTH-ENGINES-vs-NODE**, all three, and expected to be:
`NOTES.md` records that `kiln_c99_inspect` is a deliberate C port of the interpreter's `inspect`,
so the two agree by construction and the engine/engine comparison can never see a gap in what they
were both told to do. `conformance/differential/console-inspect.js` covers this area against node
and passes; these are three shapes it does not contain.

| | node | both engines |
|---|---|---|
| a symbol-keyed property | `{ Symbol(Symbol.toPrimitive): [Function: [Symbol.toPrimitive]] }` | `{}` |
| an object past the break length | broken across five lines | one line |
| an array nested under objects | `{ p0: { p1: [ [] ] } }` | `{ p0: { p1: [ [Array] ] } }` |

The third is not the depth limit itself — `[[[[1]]]]` and `{p0:{p1:{p2:{p3:1}}}}` both agree with
node — but where an array reached through objects sits relative to it.

### Confirmed fixed since

Two bugs named as known when this work started have since been fixed, and both are now
regression tests in `tests/node_reference.rs` rather than findings:

- `parseInt({toString: () => "99"})` and `parseFloat` — was `NaN` in **both** engines, is now `99`.
  This is the shape that motivated the whole exercise: while it was broken the two engines agreed,
  so `differential_native` reported a match and said nothing.
- `Math.abs({valueOf: () => 42})` and the rest of the single-argument builtin path — was `NaN` in
  the interpreter and correct in the native backend, and is now correct in both.

---

## Cost, measured

On this machine, `NativeProfile::Dev`, one program per iteration and no cache hits (every
generated program is new, which is the point). The clang column is the 1,500-case run below; the
zig column is a 60-case run of the same seed range.

| stage | clang 17 (`/usr/bin/clang`) | zig 0.16 cc |
|---|---|---|
| bytecode → C (`emit`) | 0.09 ms | 0.11 ms |
| C compile | 120 ms | 376 ms |
| link | 43 ms | 128 ms |
| whole build | 230 ms | 608 ms |
| launch and wait | 213 ms | 208 ms |
| **per iteration** | **446 ms** | **819 ms** |

A full run, for the shape of a soak: **1,500 cases in 668 s** — eleven minutes — producing

    3    DIVERGED          three instances of finding 1
    140  match             ran to completion in both, byte for byte identical
    795  native-stopped    interpreter finished, native refused at run time
    562  both-stopped      both refused, agreeing on what they printed first

Two things are worth taking from that table.

**Discovery picks the slow compiler here.** `Toolchain::discover` finds `zig` first on this
machine and zig cc is 2.4× slower than clang for the same work, which is the difference between
eight thousand and four thousand native cases in an hour. `KILN_FUZZ_CC` overrides it.

**Half the time is not compilation.** 213 ms of launch is not the program running — running
the same binary a second time takes 4 ms. It is macOS validating the signature of an executable it
has never seen, and there is one of those per iteration by construction. Nothing about the harness
recovers it; it is a cost of compiling a new program every time.

### So: is bolero the right tool here?

**For `structured_ir` and `memo_oracle`, clearly yes.** They run at 4,300 and 44,000 cases per
second under the RNG engine, and both have branchy state machines — an invalidation walk, a
bytecode validator — where "keep the input that reached new code" is exactly the right strategy.
`cargo bolero` drives them properly and the corpus it builds is worth keeping.

**For `differential_native`, the coverage guidance is not worth having, and this is a negative
result rather than a limitation of the harness.** At 431 ms per case the engine gets about two
executions a second. Coverage feedback earns its keep by making thousands of small decisions per
second; at two per second it is a slower random search with a corpus directory attached, and the
instrumentation costs a further 4×. That is why `bin/soak` generates through `arbitrary` over a
seeded xorshift stream instead of through a bolero engine, and why the bolero target for it
defaults to eight iterations — enough to be a smoke test in `cargo test`, and honest about not
being a fuzzing loop.

The right shape for target 1 is a soak: point `bin/soak` at a machine with a seed and leave it.
1,500 cases is eleven minutes with clang, and that one run found three divergences.

What *would* change that verdict is a cheaper native side. Batching many generated programs into
one translation unit, or comparing at the level of ADR 0020's state dumps rather than at the
process boundary, would move the cost per case by an order of magnitude and make coverage guidance
worth paying for. Neither is a change to this crate.

---

## How the harness works, and the four things it has to guarantee

The classification is `scripts/native.sh`'s, deliberately unchanged, because that script is where
the project already decided what "the same output" means and a second definition would be a second
answer to the same question. `src/engines.rs` adds only what a fuzzer meets and a hand-written
corpus does not.

    match             both engines ran to the end and printed the same bytes
    native-refused    the emitter named an instruction it does not implement — the backlog
    native-stopped    the native binary refused at run time, having printed a prefix of the
                      interpreter's output — also the backlog, and the prefix check is what
                      keeps that honest
    both-stopped      both refused, and agreed on everything they printed first
    DIVERGED          a wrong answer: see the rule below
    INVALID-C         the emitter produced a translation unit the platform compiler rejected
    NATIVE-CRASHED    a signal, or a binary that outlived the timeout

The rule underneath all of them: **an engine that stops is allowed to have printed less, and an
engine that finishes is not allowed to have printed anything different.** That is what lets a
deliberately growing native surface be reported as a backlog without giving up the ability to see a
wrong answer inside the bytes it did produce. An interpreter that refused where the native binary
ran to completion is always a defect — a compiler may not have two opinions about whether a program
has a value.

**1. Nothing `Rc`-bearing crosses a thread boundary.** `kiln-interpreter` is `!Send`; its object
model is `Rc` throughout (ADR 0007). The harness never has to defend that, because of where the
boundary is drawn: `engines::interpret` takes a `&Program` and returns a `String`, and every `Rc`
the VM allocates is created and dropped inside that one call. Nothing an `Rc` can reach is ever
named by a value that outlives the frame, so there is nothing that *could* travel even if an engine
wanted to move it. (Relatedly, the discovered C toolchain is reached through a thread-local rather
than captured, because `NativeBuilder` holds an `Arc<dyn ProcessRunner>` and a closure capturing one
is not `RefUnwindSafe` — which every bolero engine requires.)

**2. Generated programs terminate.** There is no `while`, no `do`, and no user-written loop
condition. The one loop form is `for (let i = 0; i < K; i++)` with `K` a literal in `0..=3` and an
induction variable nothing can assign to. Function bodies contain no calls at all, so the call graph
is one level deep — unbounded recursion is an error in the interpreter and a stack overflow in a
native binary, and reporting that pair a thousand times would say nothing about the compiler.

**3. Generated programs cannot reach the memory cliff.** Statement nesting is capped at two, so a
body runs at most nine times, so `x = x + x` in the innermost body multiplies a string by 2⁹ rather
than by 2²⁷. In the bytecode generator the same job is done by a hard instruction budget and a ban
on backward jumps that survives perturbation: with no back edge nothing executes twice, so the
number of doublings is bounded by the length of the code.

**4. The cheap engine screens the expensive one.** The interpreter runs first under a 50,000-step
budget, and the native side is only built for programs it finished. The two engines run the same
bytecode, so a program the interpreter finishes in a few thousand steps is one a native binary
finishes quickly too — and a native binary has no step limit, no supervisor, and no ceiling short
of the machine's. A 20-second timeout on the child is belt and braces; if it ever fires that is
itself a finding. Output goes to files rather than pipes, which removes the deadlock where a child
fills the pipe buffer while the parent waits for it to exit.

## Generators, and two measured mistakes

Both were caught by `tests/generator_health.rs`, which exists because a fuzz target that passes
because every case was empty is the same hollow green `conformance/README.md` refuses elsewhere.
It asserts a mean, not a maximum, and it is the reason either mistake was noticed at all.

**`Vec<T>: Arbitrary` produces a mean of one element.** `arbitrary` reads a bool before each
element and stops on the first false, so the length is geometric with p = ½. Generated histories
averaged **1.0 operations** — the memo oracle was running a one-step history a hundred thousand
times. `Many<T, MAX>` reads a length byte instead and then that many elements: mean 11.9.

**Truncating at render time throws the entropy away.** The first version let `arbitrary` build an
expression tree of any depth and cut it off at depth three on the way out. Four kilobytes of input
produced a 264-byte program: everything below the cut was generated, paid for and discarded, and
more input made no difference. `Nested<T, MAX>` bounds depth where the value is *made*, so the same
budget is spent on breadth. Mean program size went from 216 to 730 bytes, and loops, `try`/`finally`
and nested objects started appearing at all. Related: `Roomy<T>` declares no upper `size_hint`,
because bolero's driver picks a value's byte budget from that hint and a recursive type's derived
hint is a few dozen bytes.

**The grammar had no value that answers back during conversion.** It produced arrays and plain
objects, and that gap is exactly why the differential run found `Math.abs([0])` and missed
`Math.abs({valueOf: () => 42})` — the more general bug, and the one that was broken for longer. An
array converts through the same ToPrimitive machinery, but through the *built-in*
`Array.prototype.toString`, so a path that skips ToPrimitive and reads an internal slot instead
still gets an array right often enough to look correct; a user method cannot be skipped and then
guessed. `Expression::Coerce` now generates ten shapes — `valueOf`, `toString`, both disagreeing,
either returning a non-primitive, neither returning a primitive, a non-callable `valueOf`, and
three `Symbol.toPrimitive` forms — paired in the same node with one of twenty-four consumers, so
`Number(o)`, `parseInt(o)`, `` `t${o}` ``, `o * 2`, `o < 1` and `o == 1` are reachable in one draw
rather than when the tree happens to put them together. Pairing them is the point: a shape the
grammar *can* emit once in ten thousand programs is not coverage, and
`coercion_shapes_appear_often_enough_to_be_coverage` asserts a floor per shape and per consumer
rather than in aggregate, because ten variants that fail differently can hide behind one popular
sibling in any total. Measured over 4,000 programs: 3,785 carry a coercion object, the rarest shape
appears 408 times per thousand, the rarest consumer 165. Findings 4 through 8 all came out of it.

One deliberate distortion is worth stating. `Expression::Member` and `Expression::Computed` render
their receiver as `((expr) ?? {})`. Without the guard, three quarters of every generated program
died on `undefined.p0` and the corpus measured the TypeError path and nothing else; with it, 64%
run to completion. Uncaught errors have not been lost — they come from `Statement::Throw`, which is
where the remaining third still stops.

## Layout

    fuzz/
      src/lib.rs          Many, Nested, Roomy — the generator combinators, and why each exists
      src/js.rs           a grammar of valid, terminating, deterministic JavaScript
      src/ir.rs           a small compiler to well-formed kiln-ir, plus single-field perturbations
      src/engines.rs      the two-engine comparison, the classification, and the screening budget
      src/node.rs         node off PATH: one process per program, or sixty-four in one
      src/node_batch.js   the batched reference, which formats nothing and hands node's own
                          `console` a Writable instead of fd 1
      src/triage.rs       the three-way classification, and the rule it judges by
      src/oracle.rs       the memo property, ported from crates/compile/kiln-db/tests/oracle.rs
      src/bin/soak.rs     the long-running modes; the source of both cost tables above
      src/bin/differ.rs   one file through all three, for reducing a reported divergence
      tests/              the five targets
      findings/           reduced reproductions, with what node says about each
      examples/           entropy_probe: how many bytes one generated program consumes

`crates/compile/kiln-db/tests/oracle.rs` is unmodified. `src/oracle.rs` carries the same property, the same
frontend and the same text pool behind a coverage-guided generator, which is the one thing proptest
cannot give it: most branches of an invalidation walk need a *particular* sequence of edits and
reads to reach at all, and random histories reach them in proportion to luck.
