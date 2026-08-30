# `fuzz/` — differential fuzzing for kiln

kiln's central architectural claim is that **two independently written engines produce
byte-identical output from the same bytecode**: the Rust VM in `kiln-interpreter`, and the C that
`kiln-backend-c` emits and a platform compiler turns into a native binary. `scripts/native.sh`
checks that claim over a hand-written corpus, and `conformance/README.md` says exactly what a
corpus costs: *"a real program composes features in combinations nobody wrote a test for"*. This
crate writes those programs.

It found three on the day it was built — one root cause, reached three ways, in 1,500
generated programs. See **Findings** below.

## Contents

| target | what it compares | measured cost per case |
|---|---|---|
| `differential_native` | interpreter stdout vs native stdout, byte for byte | **446 ms** (clang) / **819 ms** (zig) |
| `structured_ir` | `kiln_backend_c::validate` against the interpreter's own idea of well-formed | 232 µs |
| `memo_oracle` | a memoized query engine against a from-scratch one | 23 µs |
| `generator_health` | the generators themselves, against being empty | 7 µs |

Everything is standalone: `fuzz` is excluded from the root workspace (`exclude = ["fuzz"]`,
ADR 0004) and carries its own lockfile, so bolero, libFuzzer and a C++ runtime never enter the
compiler developer's `cargo check --workspace` loop.

## Commands

Nothing below needs nightly. Sanitizers are not used and are not wanted here — the bugs being
looked for are wrong answers, not memory errors, and every engine is safe Rust.

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

`--sanitizer NONE` is required, and it is the reason nightly is not: cargo-bolero's default
`--sanitizer address` needs `-Zsanitizer`, which is nightly-only, and ASan finds nothing in safe
Rust. Without the flag the build fails on the pinned 1.97.1 toolchain.

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

# The long-running mode for the native target. Prints the outcome distribution, the reason
# distribution, and the measured cost breakdown; exits non-zero on any defect. Eleven minutes,
# and the run that produced finding 1.
cargo run --release --bin soak -- --cases 1500 --seed 101

# Same generator, no C toolchain: ~15,000 cases/second. This is how the generator was tuned.
cargo run --release --bin soak -- --cases 5000 --interpret-only
cargo run --release --bin soak -- --cases 5000 --lower-only

# One file through both engines, which is what you use on a reported divergence.
cargo run --release --bin differ -- findings/tonumber-on-an-array.js

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
      src/engines.rs      the comparison, the classification, and the screening budget
      src/oracle.rs       the memo property, ported from crates/kiln-db/tests/oracle.rs
      src/bin/soak.rs     the long-running native mode; the source of the cost table above
      src/bin/differ.rs   one file through both engines, for reducing a reported divergence
      tests/              the four targets
      findings/           reduced reproductions, with what node says about each
      examples/           entropy_probe: how many bytes one generated program consumes

`crates/kiln-db/tests/oracle.rs` is unmodified. `src/oracle.rs` carries the same property, the same
frontend and the same text pool behind a coverage-guided generator, which is the one thing proptest
cannot give it: most branches of an invalidation walk need a *particular* sequence of edits and
reads to reach at all, and random histories reach them in proportion to luck.
