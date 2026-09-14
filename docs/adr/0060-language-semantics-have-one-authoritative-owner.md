# ADR 0060: Observable language semantics have one authoritative owner

## Status

Proposed, 2026-09-13.

Generalises ADR 0025 (language semantics that need an engine are linked), ADR 0029 (mature Rust
implementations at existing seams), ADR 0048 (`node:path` is one implementation used by both
engines) and ADR 0058 (runtime engines are Rust behind a coarse provider ABI). Partially
supersedes ADR 0043's statement that a built-in member's behaviour always remains separately
implemented by the two engines.

## Context

Kiln has two execution engines with different value and heap representations. Shared bytecode and
differential corpora keep them close, but equality of test output is weaker than equality by
construction. JSON parsing, number conversion, regular expressions, BigInt, Temporal and
`node:path` have each demonstrated the better shape: one semantic implementation, called directly
by the interpreter and through a coarse native-provider adapter by emitted code.

Independent implementations are sometimes unavoidable at the representation boundary. Object
allocation in the interpreter cannot be the C heap's object allocation, and a property-cache hit
must use each engine's own layout. That fact does not justify implementing grammar, coercion,
ordering, error selection or nontrivial algorithms twice.

The phrase "one semantics" needs an ownership rule. Without one it can mean either shared tests,
shared names, shared helper libraries, or an actual single implementation, and duplicate paths
return as soon as a boundary is inconvenient.

## Decision

**Every observable semantic operation has one named authoritative owner.**

An ownership inventory records each operation or coherent family as one of:

1. **Shared semantic core.** Pure or allocation-free Rust called directly by the interpreter and
   through the provider ABI by native code.
2. **Shared engine.** A stateful or nontrivial Rust implementation exposed through coarse
   operation-level calls and opaque handles, following ADRs 0025, 0034 and 0058.
3. **Generated semantics.** One declarative operation or member description generates
   representation-specific interpreter and native adapters.
4. **Representation operation.** Each engine implements its own storage mechanics under a shared
   observable contract: allocation, slot access, write barriers, shapes, rooting and
   materialisation belong here.
5. **Temporary duplicate.** Two implementations remain during migration. The inventory names both,
   the reason, the differential authority and the deletion condition.
6. **Deliberate refusal.** The unsupported operation and the source-attributed refusal are named;
   absence or silent `undefined` is not a category.

Shared code owns grammar, Unicode and calendar rules, numeric conversion, coercion decisions,
property-key conversion, ordering rules, error classification and other algorithms whose result
does not depend on an engine's heap layout. Representation adapters own conversion to and from
their local values and materialisation of result objects.

The provider boundary remains coarse: one crossing per semantic operation, never one per element,
character, property or callback. Moving code into Rust is not unification if the ABI fragments the
algorithm into a chatty sequence or permits the two callers to impose different observable order.

An optimization may replace the authoritative generic operation only when its guard names the
equivalence invariant and its failure resumes at a point that cannot duplicate or omit an
observable effect. The generic operation remains the semantic fallback.

## Enforcement

- The ownership inventory is checked in the gate for unclassified new built-ins, bytecodes and
  provider exports.
- Generated tables use compile-time completeness checks where the Rust type system can express
  them.
- Every shared operation has adapter tests for both engines, including its error path.
- Temporary duplicates have differential tests and a named deletion condition; an unbounded
  duplicate is an architecture regression.
- A provider export is declared once and generates or verifies its Rust symbol, C declaration,
  feature membership, ownership convention and ABI identity.

## Provider binding generation

The provider's Rust `extern "C"` declarations remain the ABI authority. `cbindgen` is the preferred
candidate for deriving the C header from those declarations because its direction matches the
ownership in this decision: Rust API to C declarations. It is adopted only after a bounded probe
covering a scalar operation, an opaque handle lifecycle, a borrowed byte range, an error-returning
operation and a callback-bearing operation proves all of the following:

1. output is deterministic and produces a clean checked-in diff;
2. names and primitive representations exactly match the existing ABI;
3. target and feature selection cannot silently change the header;
4. the package script can compare the generated declaration surface with `PROVIDER_EXPORTS`;
5. using it does not put a source parser or host `libclang` on an ordinary crate build path.

If the probe passes, `cbindgen` is pinned as a repository tool and its generated C header is
committed and checked by the gate. Header generation does not replace symbol-table verification or
the provider ABI fingerprint: a declaration proves neither that the static library exports the
symbol nor that its semantics match the artifact.

`bindgen` is not considered because it generates Rust declarations from a C authority, the reverse
of this decision. Diplomat is not adopted: its multi-language ergonomic binding layer is broader
than Kiln's internal scalar-and-opaque-handle C ABI and would place another generated ABI between
the declaration and emitted C. Rust-to-Rust stable-ABI crates are likewise outside this boundary;
the shipped contract is C and static linking.

## Test mechanisms

The existing `proptest` dependency checks semantic-core laws and adapter equivalence with shrinking
and persisted regressions. The fuzz workspace's `arbitrary` dependency produces structured inputs
for grammar, opaque-handle lifecycle and provider-adapter fuzzing. A successful result comparison
includes the error kind and position, ownership transition and observable ordering where those are
part of the operation; comparing only returned payload bytes is insufficient.

## Consequences

- Differential testing remains essential at representation adapters and optimization boundaries,
  but it is no longer the preferred way to reconcile two algorithm implementations.
- Small operations may remain engine-specific when sharing them would require a more expensive
  boundary than the operation itself. They must be classified as representation operations or
  generated from one description rather than silently duplicated.
- ADR 0043's one-surface rule remains valid. Its claim that behaviour must always be separate is
  superseded as a member gains a shared or generated owner.
- ADR 0058 remains the concrete decision for JSON and `parallelMap`; this ADR makes its ownership
  rule repository-wide.
- The existing `kiln-native-number-ffi` name no longer describes the provider and must be renamed
  as ADR 0025 already requires. The rename itself is implementation work, not a new decision.

## Rejected alternatives

**Parity by corpus alone.** A corpus samples behaviour. It cannot prove two parsers, coercion
algorithms or member descriptions are the same outside the sample.

**Put all semantics across FFI.** Fine-grained calls would erase native performance and create
ordering and ownership hazards. Sharing is chosen at an operation boundary, not a source-language
token boundary.

**Make one heap representation serve both engines.** This would couple interpreter iteration speed
to native layout and is not required to give an operation one semantic owner.

**Generate the Rust ABI from a hand-written C header.** That makes C syntax the authority for a
Rust-owned provider and leaves feature membership, semantic ownership and the exported archive to
be reconciled elsewhere.
