# ADR 0056: The SSA Value Graph and Verifier

## Status

Accepted.

## Context

The Kiln native backend (`kiln-backend-c`) emits C99 translation units from Kiln stack bytecode.
Until now, emission was a direct transliteration of stack-machine instructions (`Push`, `Pop`, `Dup`, `LoadLocal`, `Binary`, etc.), supplemented by 28 ad-hoc peephole fusers (`can_fuse_*` matchers).

This architecture faces structural limitations:
1. **Memory-bound operand stack**: Because `kiln_c99_op_call(host, stack, &sp, ...)` passes `&sp` to an out-of-line runtime helper, the entire `stack[]` array and its stack pointer `sp` are address-taken. Clang cannot register-allocate the operand stack across the body and spills intermediate values to L1 cache on every instruction.
2. **Fragile and narrow peephole fusion**: Each fuser matches a fixed 2-to-4 instruction sequence (e.g. `LoadLocal`, `LoadLocal`, `Binary`, `StoreLocal`). Any divergence in bytecode sequence (such as an uninitialized guard check, a label, or an expression discarded with `Pop`) disables the fuser entirely.
3. **Impediment to static optimization**: Essential compiler passes—common subexpression elimination, loop-invariant guard hoisting, induction-variable range proofs, and static speculative cloning—cannot be performed over linear stack bytecode without recovering data dependencies on every pass.

As outlined in `docs/architecture/fastest-native-path.md` and `docs/architecture/fastest-native-plan.md` (Bet #1), we need an intermediate representation between bytecode and C emission: a Control Flow Graph (CFG) of Static Single Assignment (SSA) values, verified against strict architectural invariants before optimization and lowering.

## Decision

We introduce an SSA **Value Graph** and an accompanying **Graph Verifier** into `kiln-backend-c`.
The bytecode format and interpreter remain unchanged (ADR 0007, ADR 0043). The C backend abstract-interprets stack bytecode into the Value Graph before optimization and emission.

### 1. Representation: Turboshaft-Style Linear Value Graph

Rather than an unconstrained "sea of nodes" (which complicates code generation and schedule control), the Value Graph uses a Turboshaft-style linear buffer of operations partitioned into basic blocks:

- **Values and Operations (`ValueId`, `Operation`)**: Each operation produces an immutable SSA value identified by a dense 32-bit index (`ValueId`). Stack manipulation instructions (`Dup`, `Pop`, `Swap`) disappear entirely; data flow is expressed solely through value references.
- **Basic Blocks (`BlockId`, `BasicBlock`)**: Code is divided into single-entry, single-exit basic blocks ending in explicit terminators (`Jump`, `Branch`, `Return`, `Throw`).
- **Phi Nodes**: Merging values across distinct predecessor blocks is performed via explicit `Phi` operations at block headers.
- **Stable Site IDs**: Every object allocation, property read/write, and function call site receives a deterministic, stable 32-bit `SiteId` carried through graph transformations into emitted C.
- **Explicit Exceptional & Suspension Edges**: Exception handlers (`PushHandler`/`PopHandler`) and coroutine resume points (`Yield`/`Await`) are modeled as explicit control flow edges rather than ambient stack state.

### 2. Invariants that Bound the Design

The optimizer must remain small, fast, and correct by construction. The following six invariants govern the graph and its transformations:

1. **Two clones per region, never more**: Region versioning produces at most one speculative fast clone (unboxed registers) and one generic fallback clone (identical to unoptimized C). Deoptimization exits occur only at loop headers or before call sites where state can be boxed cleanly into locals.
2. **Integers never touch the heap**: The `int` type exists solely inside a region. Any value escaping a region or stored into an object/array is boxed as an IEEE-754 `double`. No tagged integer representation exists in the runtime or heap.
3. **A bounds check is never removed**: Range proofs on induction variables may eliminate redundant comparisons or lower checks to an unsigned branch, but the check itself is never eliminated.
4. **Effects are two-valued**: Every operation is classified as either `Pure` (no side effects, idempotent) or `Unknown` (can throw, mutate state, or invoke unknown code).
5. **A guard names its invariant**: Every speculative assertion is tied to an explicit entry in an invariant table naming the language rule it relies upon.
6. **Nothing is proven dead statically**: Memory reclamation is managed at the turn boundary (turn nursery, ADR 0041); escape analysis is local and restricted to scalar replacement.

### 3. Graph Verifier

To prevent subtle optimizer bugs and code generation errors, the graph is validated by a strict **Graph Verifier** in debug builds and during CI gate execution (`scripts/gate.sh`):

- **SSA Dominance**: Every operand `ValueId` must strictly dominate its use site, with loop-carried dependencies mediated by `Phi` nodes.
- **Type Consistency**: Binary and unary operations must consume operands matching their expected type signature or generic `kiln_val`.
- **Control Flow Validity**: Predecessor and successor lists must be symmetric; every basic block must have a valid terminator.
- **Deoptimization Maps**: Every speculative exit or deopt point must supply a complete, sound mapping from virtual SSA values back to local variable slots (`locals[]`).
- **Handler Nesting**: Exception handler scopes must form properly nested intervals with valid handler targets.

The verifier is verified against hand-corrupted test graphs to guarantee that invalid mutations fail early with descriptive diagnostics.

### 4. Code Generation from the Graph

Lowering the Value Graph to C produces clean, idiomatic scalar code:
- SSA values become C local variables (`kiln_val v12`, or `double d12` when typed `Number`, or `int64_t i12` when typed `Int`).
- The automatic operand stack `stack[max_stack]` is eliminated.
- The `locals[]` array is preserved only for variables captured in closures (cells) or preserved across suspension frames (generators/async).
- Arguments at call sites materialize into small, stack-allocated arrays directly at the point of invocation, avoiding whole-body address-taking.

## Consequences

- **Performance**: Eliminating the address-taken operand stack enables Clang to keep hot loop variables in CPU registers, removing 12 memory spills per loop iteration on benchmarks like `loop.mjs`.
- **Simplification**: The 28 hand-written peephole fusers (`can_fuse_*`) are rendered obsolete and replaced by principled graph transformations and an expression-tree emitter.
- **Verification**: The Graph Verifier acts as an oracle catching compiler regressions before C compilation.
- **Backward Compatibility**: Functions that do not undergo speculative optimization compile identically to today's verified output, preserving 100% bit-for-bit parity across `conformance/native` and Test262.
