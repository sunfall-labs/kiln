# Type-directed native code

Status: open

## Why

The point of kiln's type checker is better native code: a program's author wrote down what V8
has to rediscover at runtime (`docs/architecture/fastest-native-path.md`, "TypeScript
annotations"). The checker now matches tsc on 91% of TypeScript's own suite, and its answers are
computed on every TypeScript `run` and `build`. Almost none of them reach the code.

What reaches the C backend today (2026-10-08):

- The checker exports one `TypeClass` per parameter and per function return
  (`kiln-type-checker/src/snapshot.rs`, `source_facts`, `FactSelection::FunctionContracts`).
- `kiln_ir::apply_source_types` keeps Number, Boolean and String and drops the rest
  (`kiln-ir/src/source_types.rs`, `coarse`).
- One reader: `candidate_number_parameters_with_profile` (`kiln-graph/src/graph/analysis.rs`),
  which makes a `number` parameter a guarded unboxed guess at entry (plan step 3.1).

Lost on the way:

- Every union (`number | undefined`, `1 | 2`), and plain `boolean`: `export_type` tests
  `BOOLEAN_LITERAL`, which the `boolean` union does not carry, so `x: boolean` exports as
  `Composite`.
- Objects, arrays, tuples, classes and functions, all `Composite` with no shape.
- Return types: attached to every function, read by nothing but the build identity.
- Boolean and string parameters: attached, read by nothing.
- Expression types, resolved call signatures, property types and declared class layouts: the
  checker computes or can compute them; nothing carries them.
- Projects without a `.ts` file are never checked, so JSDoc-typed JavaScript gets nothing.

The syntactic annotation path that step 3.1 first used is dead for `.ts` files: production parses
TypeScript transformed to JavaScript, so the executable HIR carries no annotations.

## Rules

These come from `fastest-native-plan.md` ("Do not") and ADRs 0016 and 0056; they do not change.

- A TypeScript type is a guess, never a proof. Every use is guarded; a failed guard runs the
  generic clone. `.d.ts` files and `any` casts lie, and the program must still be right.
- Every speculative path has an off switch, an exit mode (`KILN_SPECULATE=exit`) and a
  differential run, and its guard names a row in `SPECULATION_INVARIANTS`.
- Measured, with checksums, in a worktree nobody else writes, load below 1.5.

## Plan

Each step lands with a measurement against the step before it.

0. **Measure.** Today nothing can say whether types changed the code.
   - Typed twins of the kernels that should benefit: `nbody`, `mandelbrot`, `class-poly`,
     `object-literal`, `string-ops`, `array-functional`, plus `bench/realworld/json-transform.ts`.
   - A switch that withholds source types (`KILN_SOURCE_TYPES=0`), for A/B on one binary.
   - A count of the speculations the emitter made and why (annotation / profile / inference),
     from the emitter rather than by grepping C.
1. **Use what already arrives.**
   - Export plain `boolean`, and `T | undefined` / `T | null` as their own classes.
   - Return types: seed the `typed_returns` fixpoint (`kiln-backend-c/src/direct.rs`) and let a
     `Call` whose callee declares `number` speculate like `spec_calls` does for `CallMethod`.
   - Boolean and string parameters: entry guards and twins (`body.rs` entry-guard loop),
     `is_proven_string` receivers.
   - Array parameters from the checker, not only from a profile
     (`candidate_array_parameters_with_profile` requires a profile today).
2. **Types at expressions.** Carry `type_of_expr` for the sites the backend can use: arithmetic
   operands, property-read receivers and results, method-call receivers (`spec_fields`,
   `spec_calls`, `is_proven_array`/`is_proven_string`).
3. **Shapes.** A declared class's instance layout seeds the receiver-shape cache and inline caches
   (ADR 0050); property types drive field-number speculation.
4. **Call targets.** `resolved_signature` at a call site adds known callees and method targets the
   graph cannot see (`known_callees`, `method_candidates`).
5. **JavaScript.** Decide whether a JS-only project is checked for facts (JSDoc and inference), and
   at what cost.
6. **Cost.** The check is on `run`'s critical path and nothing persists it across processes: cache
   facts by content digest.

## Done when

The typed twins beat their untyped originals under kiln native, with the speculation count saying
why, and no corpus (native, thirdparty, Test262, `KILN_SPECULATE=exit`) moves.
