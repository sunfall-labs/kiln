# ADR 0021: Value and heap representation is decided before the object model exists

## Status

**Proposed.** Like 0018, this lands entirely in files the Codex session owns — `kiln-ir`,
`kiln-backend-c`, `kiln-interpreter` — so it is a position to argue rather than a decision taken.

It decides nothing about *when* an object model is built. It fixes four properties any object model
must have, because all four are free while the native backend has no objects and expensive or
impossible afterwards, and two of them are correctness rather than performance.

## Context

`verify` in `kiln-backend-c` rejects `MakeFunction`, `Call`, `NewObject`, `GetProperty` and
exceptions. The native backend has no functions, no objects, no heap and no collector. Everything
interesting waits behind that, which is exactly why the representation questions are worth settling
now: there is no migration cost today and there is no cheap migration later.

The current boundary value is a thirty-two byte record — `{ uint32 tag; uint32 reserved; uint64
bits; const char *ptr; size_t len; }` — and it is simultaneously the internal representation and
the public embedding ABI, since `kiln_host_call_fn` takes `const kiln_value *`. Measured on
hand-assembled emitter output, that width is a direct cost: under the AArch64 procedure call
standard it travels in memory rather than registers, so every call to the generic operator
dispatcher stores and reloads it. The coupling to the embedding ABI is what makes narrowing it look
like a breaking change rather than an internal one.

Four decisions follow. Each has recent published evidence behind it, and two of them are recorded
here because *production systems shipped correctness bugs* by getting them wrong.

## Decision

**1. The internal value representation is one machine word, and it is not the embedding ABI.**

`kiln_value` and `kiln_host` remain the public embedding boundary and are not narrowed. The
representation used inside emitted code and inside the interpreter is a separate type, converted at
the boundary. Today they are the same type, and that identity — not the width itself — is what makes
the width unchangeable. Separating them costs one conversion at a boundary that already exists and
is already crossed rarely, and it buys the ability to change the internal encoding without an ABI
break, ever.

The encoding to evaluate first is float self-tagging: an invertible bitwise transform that places a
correct tag in a known position for a large range of IEEE-754 doubles, avoiding both boxed floats
and NaN-boxing's tax on the type check of every non-float value. It is implementable in portable C.
This ADR does not mandate that scheme; it mandates that the choice is made once, deliberately, and
before an object model constrains it.

**2. Shape identity is globally stable, content-addressed, and never recycled.**

Whatever names a hidden class, map, or layout must be derived from content and stable across
translation units and across links. Two named failure modes make this correctness rather than
taste:

- A shape name invented per translation unit breaks under separate compilation. Compilers targeting
  WasmGC hit exactly this: unable to forge distinct structural type identities across separately
  compiled modules, every class test degraded into two checks. Kiln emits one translation unit per
  module by design, so it is standing in the same place.
- A recycled shape identity lets a guard accept a stale layout. JavaScriptCore shipped an A-B-A bug
  where a property deleted and re-added produced a shape an inline cache still trusted, and fixed it
  by migrating to unique dictionary shapes that are never reused.

Kiln's content-addressed artifact keys make the first natural if it is designed in and awkward if it
is retrofitted. The second is simply a prohibition: identities are allocated, never reclaimed.

**3. Every emitted function that can hold a reference registers a frame and polls a safepoint at
entry.**

A per-thread linked chain of frames — previous pointer, slot base, local count, live stack height —
pushed on entry and popped on exit, with a safepoint poll at entry. This is collector-agnostic
codegen work and should be done before a collector is chosen, because every candidate collector
needs it and none of them can supply it.

The reason to state it as a decision rather than an implementation detail is that kiln has already
paid for it without knowing. The emitted `kiln_value locals[N]` and `stack[N]`, with the live height
`verify` proves at every instruction, is precisely the packed side stack that Whiffle — a Scheme-to-C
compiler — uses so that a safepoint need record only a stack pointer, with everything between base
and pointer live. Julia maintains the equivalent by hand in hand-written C with `JL_GC_PUSH`; kiln's
code generator would emit it. LLVM's statepoint machinery exists to *recover* this information from
optimized code; kiln has it explicitly and should not discard it in the name of a smaller frame.

A prior note in `NOTES.md` proposed a more elaborate scheme that kept references out of memory
except on the safepoint path. That is withdrawn. There is no published measurement showing a plain
frame chain is slow for this shape, and the figures usually cited against shadow stacks come from
security shadow stacks that instrument every call and return, which is a different mechanism
measuring a different thing.

**4. Variable-length payloads live in the collected heap.**

String bytes, array backing stores, and any other variable-length payload are allocated from the
collected heap. They are not `malloc`-backed allocations owned by a Rust type and released from a
finalizer. CRuby measured *negative* scaling with thread count when freeing many small allocations,
identically across glibc, jemalloc and tcmalloc, and had to move exactly these buffers into the
collected heap to fix it. Deciding this after the first payload type exists means rewriting it.

**5. The collector is not chosen here, and should not be chosen yet.**

Both attractive candidates are blocked today. MMTk's own status page still disclaims production
readiness and its most interesting plans live in unmerged forks; the Ruby binding is measurably
slower than the collector it replaces. Whippet is the better structural fit, is MIT-licensed C, and
is the collector Whiffle was built against — but its README lists macOS and AArch64 as not yet
supported, which is this project's development platform.

The sequence that survives that uncertainty: frame chain and safepoints first (decision 3, needed by
everything); then a narrow vtable-style collector interface, as Ruby 3.4 shipped, so the collector is
swappable; then a conservative off-the-shelf collector behind it for correctness and iteration
speed; then re-evaluate. Picking a sophisticated collector first inverts the project's priority
order.

## Consequences

- Separating the internal representation from the embedding ABI is work with no visible payoff on
  the day it lands. It is worth doing anyway, because its entire value is optionality that expires.
- Decisions 2 and 4 are prohibitions, not features. They cost nothing now and cannot be adopted
  cheaply later, which is the only reason this ADR exists in advance of the thing it constrains.
- Decision 3 makes emitted functions slightly larger and adds a poll at entry. That is the price of
  a precise collector, and it is lower here than in any system that has to reconstruct the same
  information — but it is not free, and a function proven to hold no references should be exempt.
- The object model this constrains must make the *megamorphic* path fast, not only the monomorphic
  one. Both JavaScriptCore and SpiderMonkey spent their recent object-model effort there, explicitly
  because real application code is megamorphic at many sites. A design that is fast only when a
  shape guard hits will disappoint on real programs, and ADR 0016's specialization does not change
  that.
- None of this is implementable until the native backend accepts objects. All of it constrains what
  that work may do, which is the point: these are the decisions that are cheap in exactly the window
  where they cannot yet be exercised.

## Sources

Checked 29 August 2026.

- Float self-tagging: Melançon, Serrano & Feeley, OOPSLA 2025. <https://arxiv.org/abs/2411.16544>
- Shape identity across separate compilation: Serrano & Findler, "A Snapshot of the Performance of
  Wasm Backends for Managed Languages", MPLR 2025.
- Shape identity recycling (A-B-A): WebKit, "Optimizing WebKit & Safari for Speedometer 3.0" (2024);
  SpiderMonkey, "Making Teleporting Smarter" (2025) for the megamorphic point.
- Side-stack roots and safepoints: Wingo, Whiffle design notes; Nofl/Whippet, ISMM 2025,
  <https://arxiv.org/abs/2503.16971>. Hand-maintained equivalent: de Souza Amorim et al.,
  "Reconsidering Garbage Collection in Julia", ISMM 2025.
- Payload allocation and parallel-free scaling: Wang, Blackburn, Zhu & Valentine-House, "Reworking
  Memory Management in CRuby", ISMM 2025. <https://doi.org/10.1145/3735950.3735960>
- Collector readiness: <https://www.mmtk.io/status>; <https://github.com/wingo/whippet>; Ruby 3.4
  modular GC writeup, Rails at Scale (2025).
