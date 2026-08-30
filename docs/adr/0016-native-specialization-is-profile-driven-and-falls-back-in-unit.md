# ADR 0016: Native specialization is profile-driven and falls back in-unit, not by deoptimizing

## Status

Accepted. **Verdict: observed types are a query-engine input, specialization is guarded with an
in-unit fallback, and deoptimizing into the interpreter is rejected outright.** None of it is built.
This records the shape the work has to take, and the one shape it must not, before the first
specialization is written and the choice becomes expensive to reverse.

Revised 2026-08-29 after a literature review. The review found prior art for the whole design
(Hopc), a discipline for where guards belong, a defect in the first draft's treatment of the cache
key, and evidence that recalibrates how much this buys. Sources are listed at the end.

## Context

The C backend emits every operation as a call to one generic dispatcher, `kiln_c99_binary(op, left,
right)`, which switches on a runtime operation code and on the operand tags. That dispatcher covers
all twenty-two operators including string concatenation and UTF-8 comparison, so it is far past the
inliner's size threshold. At `-O3` Clang leaves it out of line. The emitted loop therefore performs a
real call per operation, passes the thirty-two byte `kiln_value` through memory under the AArch64
ABI, and never constant-folds the operation code, so the tag tests survive into the final binary.
The stack machine that `render_program` emits is not the problem: `sp` is provably constant at every
instruction, so SROA would promote `stack[]` and `locals[]` to registers if the operations were
visible. They are not.

Measured on a scalar Collatz program, comparing hand-assembled emitter output against the same
program in C and under two engines (user time, best of three, arm64; the emitter output was
assembled by hand from `HEADER` and `emit_instruction` rather than by building the compiler, so it
is representative of the emission shape and not a benchmark of the shipped pipeline). All numbers
were taken with one compiler, which the CPython tail-call episode shows is not enough to trust:

| Variant | Time | What it shows |
| --- | --- | --- |
| Emitted shape, generic dispatcher, `-O3` | 0.23 s | today |
| Emitted shape, guarded numeric fast path | 0.13 s | identical to hand-written `double` C |
| `node` (V8) | 0.32 s | |
| `bun` (JavaScriptCore) | 0.09 s | faster than hand-written `double` C |
| Hand-written C over `int64` | 0.025 s | |

The third and fifth rows are the finding. A JIT beat correct, fully optimized double-precision C on
the same program, and the reason is not code generation. Both engines watch the values, observe that
they are small integers, and narrow the representation: `m % 2` becomes a bit test instead of `fmod`,
`m / 2` becomes a shift instead of a divide. The last row is that same narrowing performed ahead of
time.

So the gap between an AOT compiler and a JIT on this class of code is information, not optimization
quality. Kiln is unusually well placed to acquire it: the interpreter already runs on every `kiln
run`, which is a profiling tier that a JIT has to build and that kiln has had since the first slice.
Unlike a JIT's, its observations are not bound to a process — they can be keyed by content, cached,
and reused across runs and machines.

**This design has been built before.** Hopc (Serrano) combines data-flow type analysis, speculative
hint typing from usage patterns, and range analysis to emit a specialized version of a function
*plus a fallback version supporting full ECMAScript semantics*, validated against ES5.1 and Test262.
That is this ADR, implemented and evaluated, and it should be read before the guard layer is
designed. It is also the only prior art: the 2024-2026 literature contains nothing addressing
guarded specialization without deoptimization, and every academic guard design in that window
assumes a bail-out to an interpreter.

**Two results bound what this can be expected to buy.** Porffor, an untyped whole-program AOT
JavaScript compiler, beats a non-JIT interpreter by roughly 14% on the V8 suite — which is the
honest ceiling for a native backend without effective specialization, and reframes this ADR from an
optimization into the thing that makes the native backend worth having. Against that, JSC's and
SpiderMonkey's recent object-model investment went into making the *megamorphic* path fast,
explicitly because real application code is megamorphic at many sites. Whole-program monomorphization
will not cover a large fraction of real property accesses. The generic path is the common case, not
the exceptional one, and it should be engineered as such.

The tempting alternative, and the one this ADR exists to close off, is to complete the JIT analogy:
treat the daemon as a compiler thread and bail out of specialized native code into the interpreter
when a guard fails. The frame layouts appear to invite it. Both sides index `locals[]` by the same
`program.local_count`, `verify` already proves a static stack height at every instruction, and
`render_program` emits a label at every bytecode index, so a transfer point exists everywhere and the
slot mapping that register allocation destroys inside a real JIT is preserved here by construction.

That reading is half right and the wrong half is fatal. The slot mapping is preserved; the value
representation is not. `kiln_value` is a flat thirty-two byte record with five tags. The
interpreter's `Value` is a fifteen-variant enum whose `Object`, `Array`, `Closure`, `Generator`,
`Promise`, `RegExp`, `Date` and `Cell` payloads sit behind `Rc`, and they sit behind `Rc` precisely
because object identity has to survive: `a === b` asks whether two references are the same object.
Heap values therefore cannot be converted by copying. Transferring one across the boundary requires
the two sides to share a heap, which means either rewriting the interpreter onto the C
representation and giving up `Rc`, or calling into Rust for every object operation in compiled code.
The first is an enormous change to the crate that currently defines the language's semantics; the
second deletes the performance the specialization exists to obtain.

## Decision

**Specialization is guarded and falls back within the translation unit.** A specialized site tests
its assumption and, when the test fails, performs the same operation through the existing generic
dispatcher. No state is transferred, no frame is reconstructed, no heap is shared, and no
interpreter is linked into the artifact. The fallback path is code that already exists and is
already correct, which is what makes the mechanism cheap: `kiln_c99_binary` becomes the slow half of
every fast path rather than the only path. The cost is code size in the emitted unit, paid in a
place where it is measurable and bounded.

**Deoptimizing into the interpreter is rejected**, for the representation reason above. The
symmetric frame layout is real and worth preserving for other purposes — it is what makes the
differential harness able to compare native and interpreted execution at instruction granularity —
but it does not amount to a transferable frame, and an ADR that let the symmetry imply otherwise
would be inviting a shared-heap rewrite by increments.

**Guards are placed where the assumption enters, not where it is used.** When analysis cannot narrow
a type, the guessed type is marked speculative in the analysis state and everything downstream is
derived from it rigorously, so a single guard at the point of entry licenses every subsequent
operation on that value rather than each one testing again. This is the difference between one test
per value and one test per operation, and it is a property of how the analysis is written, not an
optimization applied afterwards.

**Guard sites are first-class artifacts of the emitter: addressable, dumpable, provable and
countable.** Three independent lines of recent work converge on the guard site as simultaneously the
natural state-comparison point between execution tiers, the natural proof obligation, and the natural
coverage unit. Each needs the same thing from the compiler — a stable identity per site — and each
was retrofitted painfully in the systems that wanted it. Kiln is writing its first guard now, so the
identity is nearly free today:

- *Dumpable*: a canonical, tier-invariant state dump keyed on bytecode offset, emitted from the
  interpreter and from native code at guard sites, replaces stdout comparison as the differential
  oracle. Kiln's deterministic realm removes the false-positive suppression that dominates this
  technique elsewhere.
- *Provable*: a guard plus its two paths is a refinement obligation — for all inputs satisfying the
  guard, the specialized path equals the generic path — which is dischargeable by SMT for a narrow
  slice (integer arithmetic, monomorphic property access) and blows up on anything touching the
  heap. Scope it to that slice.
- *Countable*: coverage as the set of observed `(site, type)` pairs, enumerable at compile time
  because both components are compiler-assigned, tells us which specialization claims have actually
  been exercised and with what.

**Observed types and ranges are an input, not a derived query.** `QueryEngine`'s third invariant
requires reads to be pure: the same query at the same revision returns an equal value. A runtime
observation is not derived from source and cannot satisfy that, so it enters the engine the way
source does, through a setter that moves the revision. Following `set_source`, writing an equal
profile is not an edit: it does not move the revision and invalidates nothing. Profile convergence
therefore terminates on its own, by the same mechanism that makes a stateless client safe to point
at a warm daemon. The profile type lives in `kiln-db` beside `Parsed` and `Interface` and inherits
ADR 0004's zero-dependency rule.

**A profile changes performance and never semantics.** Guards are emitted unconditionally. A stale,
absent, or hostile profile costs speed and cannot change what a program computes. This is what keeps
correctness ahead of native performance while speculating harder than a JIT would, and it is the
property that makes accepting a profile from outside the project defensible. A profile that fails to
apply must say so loudly rather than degrading silently.

**The profile is a versioned artifact of its own, and profile staleness must not invalidate
compilation.** The first draft of this ADR put the profile digest into `ArtifactKey` and treated that
as sufficient. It is not: a profile in the key means any edit that shifts observed types invalidates
the whole translation unit's objects, which taxes iteration speed — the priority ranked directly
above native performance — to buy a property that matters less. The profile is therefore a separate,
versioned, inspectable file with its own identity, and applying it tolerates staleness: a profile
collected against an earlier revision is matched onto the current one where it still fits and
ignored where it does not. Validity is established by recording the observations the compiler
actually consulted and re-checking them, so an unmatched or invalidated entry is dropped rather than
trusted. Exactly which of these identities enters the cache key is deferred to the implementing ADR,
but "digest the profile into the object key" is settled as the wrong answer.

**Profile churn does not cascade.** `Interface` is derived from source alone. A changed profile can
affect that module's code but not its interface, so it cannot reach dependents or the project
rollup. ADR 0003's early cutoff is what makes an observe-recompile loop affordable, rather than
something it has to be defended against.

**Guard reporting is a HostOp.** Reporting a failed guard back to the daemon takes the next free
operation code under ADR 0007's versioned boundary and a matching entry in the backend's `verify`
allowlist, which today admits only `WriteStdout`. It is a development-profile facility and is absent
from `native-release`.

**Whole-program specialization is confined to `native-release`.** Specializing one module against
another module's body, rather than its interface, is exactly the dependency ADR 0009's per-module
artifact split exists to avoid, and it would trade away the early cutoff the project ranks second.
Development builds stay module-local. Cross-module work and LTO belong to the release profile, which
design.md §8 already describes as the place optional LTO lives.

## Consequences

- The first specialization to write is numeric, because it is the one the current backend subset can
  express: guarded integer representation for operands a profile reports as integral and in range,
  falling back to the double path. That is the 0.13 s to 0.025 s row.
- Do not spend effort making the guarded fast path cheaper once it exists. The strongest available
  evidence — from the team behind the closest comparable system — is that removing loads from a
  successful inline-cache hit produced no measurable speedup on contemporary out-of-order cores. The
  wins are in eliminating calls, allocation and boxing, not in shaving instructions.
- Emitted units grow. Every specialized site carries both halves, and the size is proportional to
  how many distinct assumptions a profile justifies rather than to program size. The version-explosion
  and context-selection problem this creates has been treated formally for AOT basic-block versioning
  and should be read before a version limit is chosen by feel.
- The engine gains an input that is not source. Determinism is preserved by keying on it, but
  "reproducible from a checkout" now means reproducible from a checkout and a profile, and a build
  without one must remain correct and merely slower.
- A profile is content-addressed, so it is an artifact: it can be committed, shared through a
  read-only cache layer of the kind `CacheLayer::read_only` already anticipates, or shipped beside a
  package. A program can then be fast on its first execution on a cold machine, which is the one
  thing a JIT cannot do, since it has not observed anything yet. No existing system ships *type*
  feedback this way; the shipped-profile precedents carry method hotness and branch frequencies.
- The interpreter acquires a second job. It is the semantics oracle and now also the profiler, and a
  change to it can move native performance without moving native semantics. ADR 0005's bootstrap
  caution applies in a milder form: the compiler now depends on its own execution for speed, though
  never for meaning.
- None of the object, shape, or allocation half of the argument is reachable yet. `verify` rejects
  `MakeFunction`, `Call`, `NewObject`, `GetProperty` and exceptions, so the native backend has no
  object model for a shape profile to describe. When it does, shape identity must be globally stable
  and never recycled — a shape name invented per translation unit breaks under separate compilation,
  and a recycled one lets a guard accept a stale layout. Both are correctness failures, and both have
  bitten production systems.
- Rejecting deoptimization keeps the artifact free of the interpreter and keeps the two value
  representations independent, which preserves the ability to change either one without a
  synchronized rewrite of the other.

## When to revisit

The rejection of deoptimization rests on one fact and should be reopened if that fact changes: the
interpreter and the native backend hold values differently and heap values have identity. If the
interpreter ever adopts the native value representation for its own reasons — a unified heap, a
garbage collector shared with compiled code, or `Value` losing `Rc` — then the objection is gone,
the frame symmetry becomes a transferable frame, and bailing out at any instruction index becomes
the cheapest deoptimization mechanism any engine has, because `verify` already computes the metadata
that makes it possible. Until then, the guarded in-unit fallback obtains the same speculation
freedom without requiring the two sides to agree on anything.

Separately: if the emitter is ever derived from the interpreter by partial evaluation rather than
written alongside it, most of this ADR becomes moot, because the specialized and generic paths would
share a single source of truth. That is a different architecture, not an amendment to this one.

## Sources

Checked 29 August 2026. Measurements in the table are mine, on one machine with one compiler.

- Hopc, the closest prior art: Serrano, "JavaScript AOT Compilation" (DLS 2018) and
  "Of JavaScript AOT Compilation Performance" (ICFP 2021).
- Guard placement from speculative analysis state: Aigner & Marr, "Towards Ahead-of-Time
  Meta-Compilation of Dynamic Languages With an Extensible Type Analysis", ICOOOLPS 2026 — a
  position paper with no implementation; the discipline is borrowed, not the results.
- Inline-cache optimization as a false lead: Poirier, Rohou & Serrano, *The Art, Science, and
  Engineering of Programming* 10(1):6, 2025. <https://arxiv.org/abs/2502.20547>
- Version explosion in AOT basic-block versioning: Melançon, Feeley & Serrano, "Static Basic Block
  Versioning", ECOOP 2024.
- Profile staleness tolerance: Ayupov, Panchenko & Pupyrev, "Stale Profile Matching", CC 2024
  (shipped in BOLT). <https://arxiv.org/abs/2401.17168>
- Validating cached compilation against recorded assumptions: Pečimúth, Leopoldseder & Tůma,
  "Reusing Highly Optimized IR in Dynamic Compilation", ECOOP 2025.
- Guard sites as instrumentation point: Wachter et al., "DUMPLING", NDSS 2025 (state dumps);
  Kwon et al., "TurboTV", ICSE 2024 (translation validation with a deoptimization flag);
  Wienand et al., "TYPEFUZZ", NDSS FUZZING 2026 (`(site, type)` coverage).
- Megamorphic reality check: WebKit, "Optimizing WebKit & Safari for Speedometer 3.0" (2024);
  SpiderMonkey, "Making Teleporting Smarter" (2025).
- Shape identity failures: the WasmGC separate-compilation problem in Serrano & Findler,
  "A Snapshot of the Performance of Wasm Backends for Managed Languages", MPLR 2025; the
  deleted-then-re-added shape A-B-A bug in the WebKit post above.
- Compiler-version sensitivity of any single-toolchain measurement: Elhage, "Performance of the
  Python 3.14 tail-call interpreter" (2025).
