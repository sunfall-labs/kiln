# ADR 0038: A realm owns its intrinsics, and a value says which realm made it

## Status

Accepted, and implemented for the interpreter. The C backend refuses realm creation by name.
`ShadowRealm` is not implemented and is discussed under *What this does not decide*.

## Context

A realm owns its intrinsics. Two realms' `Array`s are different functions, `arr instanceof
other.Array` is false while `Array.isArray(arr)` is true, and an error thrown by realm B's
built-in is an instance of B's `TypeError` and not of A's. None of that is decoration: it is
what a realm *is*, and roughly six hundred variants of Test262 ask about nothing else.

Kiln had one set of intrinsics, and the reason was structural rather than an oversight. An
intrinsic here is not an object. `Value::Native` is a `Copy` marker compared by value, and its own
doc comment said what that costs:

> `Copy` and compared by value: there is exactly one `Error` in a realm, so two references to it
> are the same function, which is what `===` should say.

So `$262.createRealm` refused. That was the right refusal and it is worth naming why: a second
realm whose intrinsics were secretly the first's would answer several hundred identity questions
*confidently and wrongly*, which is worse than refusing and much harder to notice afterwards.
`error_prototypes` already carried the note that it was "the smallest thing a realm is, and where
a real realm object will start". This is that.

## Decision

### A realm is three fields, and everything else is deliberately shared

A realm is a global object, a set of error prototypes, and a share of the built-in mutation store.
Those are exactly the three `Vm` fields whose comments already described them as per-realm.

Everything else the execution owns is shared, and each sharing is a decision rather than an
omission:

- **The clock, entropy, timezone and locale** are ADR 0031's capabilities *of the execution*. A
  second realm takes them on the same terms. It does not get a clock of its own and it certainly
  does not reach for the host's: ADR 0015's guarantee is that nothing here reads the host clock at
  all, and "a new realm needs a real one" is exactly the plausible-sounding argument that would end
  it in a diff whose title says something else.
- **The global symbol registry** is shared because the specification shares it. `Symbol.for("x")`
  is the same symbol in every realm of an agent; that is the whole of what makes it a registry.
- **The microtask queue and the event loop** stay ADR 0033's and ADR 0028's. Realms in one agent
  share a job queue.

### The realm is part of an intrinsic's identity, and the compiler was made to say where

`Value::Native` and `Value::Callback` carry a `RealmId`, and `strict_equal` compares it. The
widening was done as its own commit, pinned to the primary realm, precisely so that `rustc` would
enumerate all 780 sites that mint an intrinsic instead of leaving them to be found later by a wrong
answer. That is the general rule this ADR wants remembered: **when identity is being split, split
the type first and let the compiler produce the worklist.** The corpus was unchanged by that
commit, measured rather than assumed.

The distribution the compiler reported is also why the rest was tractable: 279 of the sites are in
`Native::property`, and 68 in `global_object`.

### A table is realm-blind; the receiver retags what it answers

`Native::property` is a pure function of a `Copy` marker and a name. It has no receiver to ask, so
it mints in the primary realm and the caller retags from the receiver it had in hand. A value the
program *stored* on a built-in is returned untouched, because it is not an intrinsic — it keeps
whatever realm it already had.

### Everything else says which realm made it through its `[[Prototype]]`

Only the two markers and a closure carry a realm of their own. An array built in realm 1 is an
ordinary array; what makes it realm 1's is that it links to realm 1's `Array.prototype`. That is
not a shortcut, it is what the specification says the difference is, and it means the common case —
one realm — records nothing and costs nothing.

The closure is the exception because `GetFunctionRealm` needs an answer for a callable whose
`prototype` is not an object, and `GetPrototypeFromConstructor` then falls back to *that* realm's
default prototype. Seventy-seven files in the suite test that one step.

### Calling a function enters its realm, including on the error path

A call into another realm's function is a realm switch: everything the callee mints belongs to its
realm. That includes errors. A machine-raised error carries a kind and a sentence, not an object,
and is not made into an object until something catches it — by which time the realm that raised it
has been left. So it is built at the switch back, which is the only point that knows both.

### Creating a realm is a host capability

`ExecutionCapabilities::with_host_realms` is off by default. The engine publishes one hook per
realm; the `$262` object that wraps it is assembled by the conformance runner, because what a host
can do is the host's statement to make and not the engine's. A program cannot acquire a second
realm because of who happened to run it.

### The C backend refuses by name

Its global object is one copy built in C. It cannot make a second realm, so it refuses the hook
where the *program names it* rather than where the answer would be used, and the refusal says
which hook it refused. Refusing at the name is the point: a backend that returned its own realm
would diverge silently on exactly the questions that matter.

## Alternatives rejected

**Intrinsics as ordinary heap objects (design.md §5, ADR 0005).** This is the real endgame and it
subsumes the problem: if `Array` is an object, two realms' `Array`s are two objects and nothing
here is needed. It is also a much larger project than realms, it is gated by ADR 0005's bootstrap
risk, and doing it *because* a realm is wanted would be deciding the object model under pressure
from a conformance number. The design above is compatible with it: when built-ins become objects,
`RealmId` on the marker becomes the object's own identity and the retagging disappears.

**A membrane: a second `Vm`, with cross-realm values wrapped at a boundary.** Attractive because
everything unhandled at the boundary refuses rather than answering. Rejected because a wrapper is
*relative* — realm A's view of a B value and B's view of an A value are different wrappers of the
same thing — and identity then depends on caching wrappers correctly, so `other.Array ===
other.Array` becomes a property of the cache rather than of the language. An absolute realm tag has
no such failure mode.

**Tagging every heap value with a realm.** Twenty-odd payload structs and 237 allocation sites, to
express something the `[[Prototype]]` already says. It would also make the one-realm case pay for
the two-realm case at every allocation.

## `gc-arena` is not adopted here, and this is the seam it would need

ADR 0029 §6 gates `gc-arena` on a spike proving exact roots across interpreter frames and opaque
native handles, collection only at realm-safe checkpoints, explicit dynamic roots for pending
native continuations, no Rust lifetime leakage through C, and acceptable cost. It notes that the
isolated mechanics spike passed, and that the production conditions remain.

Multiple realms is genuinely the case that makes a rooting model matter, so the question was asked
here rather than deferred by default. The answer is **not yet**, for three reasons.

1. **There is no seam to swap.** Allocation in `kiln-interpreter` is 237 open-coded `Rc::new` calls
   spread through a 47,000-line file, and the reference graph is `Rc` all the way down. Adopting
   `gc-arena` means threading a `'gc` lifetime through every one of them and through `Value`
   itself. ADR 0029's own rule 1 says a Module is adopted when it sits behind an *existing* seam;
   this one would have to build the seam first, and building the seam is the great majority of the
   work whichever collector then sits behind it.

2. **Realms do not change the collection problem, they enlarge it.** Kiln already leaks intra-realm
   cycles — `Vm::release_cycles` cuts the realm roots at the end of an execution and the closure
   cycles that survive are a known, documented limitation. Cross-realm cycles are more of the same
   shape, bounded by the same execution lifetime, and `release_cycles` was extended to every
   realm's roots rather than one's. Nothing about per-realm intrinsics is newly unsound without a
   collector, so adopting one here would be motivated by proximity rather than by need.

3. **ADR 0021's prerequisites are still ahead of it, and they are collector-agnostic.** Decision 3
   — a frame chain and a safepoint poll — is needed by every candidate and can be supplied by none
   of them, and ADR 0021 explicitly sequences it first. ADR 0004's rebuild budget also bears on it:
   `kiln-interpreter` is on the rebuild path, and a proc-macro-bearing dependency on that path has
   a cost that a realm feature has no business paying.

**The seam it would need, concretely.** An allocation Interface owned by the realm rather than by
free functions: every `Rc::new(...)` in the interpreter becoming `realm.alloc(...)`, and `Value`'s
reference kind becoming an associated type of that Interface rather than `Rc` spelled 237 times.
Per-realm intrinsics is what makes that Interface natural — there is now an object that *owns* a
heap, where before there was only a `Vm` — so the next honest step toward ADR 0029 §6 is to give
the realm its allocator with `Rc` behind it and no behaviour change, and only then to try swapping
what is behind it. That is a separate piece of work with its own measurement, and it should not be
smuggled in beside a conformance gain.

MMTk and Whippet remain where ADR 0021 §5 left them: not chosen, and not chosen yet.

## What this does not decide

**`ShadowRealm`** is not decided here, only unblocked. It needs what this ADR provides — a realm
whose intrinsics are its own — plus two things it does not: an object that *is* a realm handle, and
`WrappedFunctionCreate`, the callable that copies primitives across the boundary and wraps the
callables it returns.

The second is the part worth flagging, because there is a cheap answer that is wrong. A wrapped
function looks like a bound function with a flag on it — same `typeof`, same prototype chain, same
property storage — and building it that way means every one of the three hundred `match` arms that
already handles `Value::Bound` silently accepts a wrapper as an ordinary bound function. It is a
*new value kind*, and spelling it as one is what makes the compiler point at the places that have
to decide. `feature/shadowrealm` does exactly that, with `Value::Wrapped`; this ADR records the
choice because the shortcut is the tempting one and its failure mode is invisible.

**Whether a realm may hold host capabilities the realm that made it does not.** Today it cannot:
the capabilities are the execution's and every realm sees the same ones. That is the conservative
answer and it is the only one Test262 needs. A `ShadowRealm` with narrower authority than its
creator is a real thing to want and is deliberately not decided here.

## Consequences

- The corpus moved 86,465 → 86,784 passing, 78,043 → 78,362 addressable, with `unsupported`
  falling 1,293 → 801. Every directory was diffed against a baseline taken from the same tree; no
  variant moved out of passing. Those are this decision's own figures, measured before `main` was
  merged into the branch; the merge brought the module-cycle work with it and the branch tip reads
  86,829 / 78,407, which is not all this.
- The failures that replaced refusals name real gaps rather than hiding them: cross-realm receiver
  checks on the legacy `RegExp` statics, which this engine does not implement at all, and code
  compiled by `other.Function` looking for `assert`, which is the existing gap that a script's
  bindings live in module slots rather than on the global object.
- `Value` grew a field on two variants. One realm records no prototypes it did not already have, so
  the single-realm path is unchanged; the sweep measured that before any realm existed.
- The C backend now refuses two named hooks. That list is a work queue, not a defect report, and it
  is the honest form of "this engine cannot do that yet".
