# ADR 0061: Bytecode remains the executable semantic source of the Value Graph

## Status

Accepted, 2026-09-26, as built: the value graph is constructed from bytecode and no
HIR-to-value-graph path exists. Settled by the project owner on what the code already does.

Originally proposed, 2026-09-13.

Reaffirms ADR 0007 (versioned interpreter bytecode) and ADR 0056 (the C backend
abstract-interprets bytecode into a Value Graph). Refines ADR 0056 by
requiring explicit effects and complete exceptional and suspension state. Rejects a sibling
HIR-to-Value-Graph lowering path.

## Context

ADR 0056 introduced the SSA Value Graph between stack bytecode and C emission. Once that graph
became the native backend's real optimizing representation, an apparent simplification became
available:

```text
owned HIR
   +-- bytecode --> interpreter
   `-- Value Graph --> native backend
```

This would preserve information before stack lowering and avoid recovering SSA. It would also
create two executable semantic lowerings. Evaluation order, coercions, abrupt completion,
exception regions, suspension state and source attribution would have to remain equivalent across
the bytecode compiler and the graph compiler. That is the class of structural duplication ADR
0060 rejects.

The current direction is supported by production JavaScript engines:

- V8 Maglev performs a prepass over Ignition bytecode and builds its SSA graph from that bytecode.
  Its graph construction explicitly tracks abstract interpreter state and loop phis.
  <https://v8.dev/blog/maglev>
- SpiderMonkey Warp snapshots bytecode and inline-cache data, then translates them into MIR before
  optimization and LIR lowering. Bailout reconstructs baseline-interpreter state.
  <https://firefox-source-docs.mozilla.org/js/how-we-optimize.html>
- JavaScriptCore describes bytecode as the engine's source of truth: LLInt executes it, Baseline
  compiles it, and DFG/FTL parse it into DFG IR. Its account of speculative compilation explains
  why bytecode indices and liveness are useful authorities for OSR state.
  <https://webkit.org/blog/9329/a-new-bytecode-format-for-javascriptcore/>
  <https://webkit.org/blog/10308/speculation-in-javascriptcore/>

Recent research points toward reducing tier drift by generating tiers from bytecode semantics,
not by adding a second lowering from the source IR. Deegen specifies bytecode semantics once and
generates an interpreter and baseline JIT from them; its optimizing tier remains future work, so it
does not prove that an optimizing graph should be generated automatically, but it supports
bytecode semantics as the shared executable authority.
<https://compilers.stanford.edu/software/deegen/>
<https://arxiv.org/abs/2411.11469>

These systems are tiered JITs and Kiln is primarily an interpreter plus AOT compiler, so their
choice is evidence rather than authority. Kiln has an additional reason to retain the boundary:
the bytecode is cached, versioned, parser-independent and already serves as its differential
coordinate system.

## Decision

**The Value Graph continues to be constructed by verified abstract interpretation of versioned
Kiln bytecode. There is no independent HIR-to-Value-Graph executable lowering.**

The bytecode defines evaluation order, generic observable operations, exception regions,
suspension points and source locations. The Value Graph makes its data flow, control flow, types,
effects and optimization opportunities explicit without changing those semantics.

Information needed only for optimization may accompany bytecode as versioned, non-semantic
metadata when all of the following hold:

1. Ignoring it produces the same program behaviour.
2. It is derived deterministically from owned compiler input.
3. The native cache identity includes its version and content where relevant.
4. The graph verifier rejects metadata inconsistent with the bytecode.

Examples include stable site identities, source-level hints, profile observations and conservative
proof summaries. Metadata may improve a proof; it may not supply an operation or ordering edge
absent from bytecode.

## Explicit effects

ADR 0056's `Pure | Unknown` classification is the safe starting point, not the permanent effect
model. Each graph operation carries a conservative combination of effects sufficient to answer at
least:

- may throw;
- may call user code;
- may allocate or reach a collection boundary;
- reads or writes realm heap state;
- reads or writes host state;
- may suspend;
- reads clock, entropy or locale capabilities.

An unknown operation has every effect. Optimization may remove, reorder, clone or move an
operation only when its effect set and alias rules permit it. Parallel-safety proofs consume the
same effects instead of maintaining an unrelated syntactic allowlist.

The initial representation is `bitflags::bitflags!` over an explicitly sized integer. `UNKNOWN` is
defined as the union of every declared observable effect, and a test fails when adding an effect
does not expand it. Deserialisation and construction at untrusted boundaries reject unknown bits;
forward-compatible raw bits are not useful inside a versioned compiler IR. A richer effect or
memory-SSA system requires evidence that the bitset prevents a useful optimization; it is not
adopted pre-emptively.

## Exceptions, suspension and reconstruction

Exceptional successors, handler nesting, suspension exits and resume entries are first-class graph
control flow as ADR 0056 requires. Every throw, suspend, speculative exit and generic fallback has
a complete reconstruction map to a bytecode boundary. The map identifies locals, live operand
stack values, handler state and suspension-frame values.

No optimization may exit between an observable effect and the bytecode state update that records
its completion. A fallback must neither repeat the effect nor skip its result. The Graph Verifier
checks completeness, dominance and boundary validity of reconstruction maps.

Bytecode offsets remain the common coordinates for diagnostics, differential state comparison,
profile attachment and future OSR or deoptimization. Generated C source locations may supplement
them but do not replace them.

## One semantic description, later

Keeping bytecode as the graph input does not require hand-writing every tier's implementation
forever. Following ADR 0060 and the direction demonstrated by Deegen, a later project may describe
an instruction's generic semantics once and generate:

- interpreter dispatch scaffolding;
- Value Graph construction scaffolding;
- effect declarations;
- verifier rules;
- generic native fallback adapters;
- structural cross-engine tests.

Generation is adopted instruction family by instruction family. The generated artifact must retain
an explicit, reviewable bytecode contract; the interpreter implementation itself does not become
an opaque compiler frontend.

## Graph storage and verification tools

The Value Graph continues to own its IR rather than adopting Cranelift IR. The small
`cranelift-entity` crate is evaluated separately as a possible implementation tool for typed dense
indices and primary or secondary maps (`ValueId`, `BlockId`, operation tables and verifier side
tables). It is adopted only if a source comparison shows that it removes bespoke indexing code
without pulling Cranelift code generation into the dependency graph, weakening serialisation, or
making graph diagnostics less direct. Existing correct typed IDs are not rewritten merely to use
a library.

`proptest` generates small valid bytecode programs and graph mutations that exercise dominance,
effects, handlers and reconstruction maps, with shrinking to a readable failing program. The fuzz
workspace uses `arbitrary` for coverage-guided structured bytecode and graph inputs. `insta`
continues to snapshot emitted C where textual lowering is the contract; snapshots do not replace
semantic or verifier properties.

Cranelift and `regalloc2` are not dependencies of this decision. C remains the sole scheduled
native backend and the host compiler owns instruction selection and register allocation. The
backend-neutral Value Graph preserves the option to evaluate direct machine-code emission later,
but importing its backend machinery now would create a second code-generation and platform surface
without removing the C path. Equality-saturation frameworks such as `egg` or `egglog` are also not
adopted: Kiln's immediate requirement is a small, effect-aware, verifier-bounded optimizer with a
strict compilation-latency budget.

## Consequences

- ADR 0007's parser-independent interpreter and cache boundary remains intact.
- ADR 0056's graph builder remains a real compiler frontend and must preserve bytecode state
  precisely; its reconstruction work is accepted as the cost of one executable semantic lowering.
- The Value Graph remains backend-neutral even though C is its only scheduled backend. A future
  backend consumes the graph rather than adding another source-language lowering.
- Optimizer-only information that bytecode cannot express travels as verified metadata rather than
  motivating a sibling graph compiler.
- The `fastest-native-plan.md` "Later" item about generating tiers from one instruction description
  remains valid and is governed by ADR 0060; it does not authorize bypassing bytecode.

## Rejected alternatives

**Lower HIR independently to bytecode and the Value Graph.** This preserves source information but
duplicates executable lowering decisions at exactly the point where evaluation order and abrupt
completion become observable. Verified metadata captures optimization-only information without
that semantic fork.

**Use the Value Graph as the interpreter format.** An SSA graph optimised for analysis, explicit
effects and C emission is a worse compact cached interpreter format. It would couple the compiler
user's fastest loop to native-optimizer representation changes.

**Treat production JIT architecture as proof.** V8, SpiderMonkey and JavaScriptCore have profiling
and OSR requirements Kiln does not currently share. Their convergence makes bytecode-derived graph
construction a supported direction; Kiln's correctness and iteration boundaries are the actual
reason for the decision.

**Adopt Cranelift IR or `regalloc2` while still emitting C.** This duplicates backend machinery and
makes the Value Graph answer to an implementation that does not own the shipped code path.

**Use equality saturation as the first optimizer framework.** Unrestricted rewrite search is a
poor default for effectful JavaScript, precise reconstruction points and predictable native-build
latency. A later measured optimization may reconsider it for a pure bounded subgraph.
