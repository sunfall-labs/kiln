# ADR 0052: Zero-spill calling convention, branch prediction hints, and fused codegen

## Status

Accepted.

## Context

Kiln compiles JavaScript/TypeScript bytecode into C99 translation units via `kiln-backend-c`.
Analysis of engine engineering principles (documented notably in Wasmi 2.0's engine overhaul)
revealed three structural inefficiencies in Kiln's emitted C code:

1. **Calling Convention Over-subscription**:
   Internal function bodies (`kiln_body_fn`) took 9 parameters:
   ```c
   int fn(kiln_host *host, kiln_value *result, kiln_value *upvalues,
          kiln_value receiver, const kiln_value *args, size_t argc,
          int depth, kiln_value *thrown, kiln_generator *frame);
   ```
   Because `kiln_value` is a 16-byte struct, passing `receiver` by value consumed two argument registers.
   On System V AMD64 (which provides 6 GPR argument registers: `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`),
   `argc`, `depth`, `thrown`, and `frame` were spilled onto the stack frame on every function call.
   On AArch64 (8 GPRs), `thrown` and `frame` spilled.

2. **Compare-and-Branch Value Boxing**:
   Relational comparisons (`<`, `<=`, `>`, `>=`) constructed a boxed 16-byte `kiln_value` boolean,
   pushed it onto `stack[sp++]`, immediately popped it via `stack[--sp]` in `JumpIfFalse`/`JumpIfTruthy`,
   and called `!kiln_c99_truthy(...)` (which inspected the tag and unpacked bits).

3. **Missing Branch Predictor Hints**:
   Hot numeric and type fast paths (`if (left.tag == KILN_VALUE_NUMBER && ...)`) lacked compiler
   branch weights. Clang and GCC could not guarantee linear fall-through for the hot path,
   frequently placing cold bailout and deopt code adjacent in cache lines.

## Decision

1. **Zero-Spill Function Signature**:
   Consolidate execution environment fields (`host`, `thrown`, `frame`, `depth`) into a `kiln_call_context`:
   ```c
   typedef struct kiln_call_context {
       kiln_host *host;
       kiln_value *thrown;
       struct kiln_generator *frame;
       int depth;
   } kiln_call_context;

   typedef int (*kiln_body_fn)(kiln_call_context *ctx, kiln_value *result,
                               kiln_value *upvalues, const kiln_value *receiver,
                               const kiln_value *args, size_t argc);
   ```
   All 6 parameters fit directly into registers on SysV AMD64 (`rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`)
   and AArch64 (`x0` through `x5`), eliminating stack argument spills entirely on function invocation.

2. **Fused Compare-and-Branch Emission**:
   When a comparison instruction is immediately followed by a conditional jump, emit a direct C branch:
   ```c
   if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER)) {
       if (!(a < b)) goto kiln_label_target;
   } else {
       answer = kiln_c99_binary(operator, left, right);
       if (!kiln_c99_truthy(answer)) goto kiln_label_target;
   }
   ```
   This skips `kiln_c99_boolean` creation, stack push/pop, and runtime truthiness dispatch.

3. **Compiler Branch Predictor Annotations**:
   Define `KILN_LIKELY` and `KILN_UNLIKELY` using `__builtin_expect` when targeting GCC or Clang.
   Annotate hot type-guard checks and error returns throughout the emitted code and prelude.

## Consequences

- Generated C translation unit size and compilation time decrease.
- Function call overhead is reduced by avoiding stack frame writes for arguments.
- Hot loop branch conditions evaluate directly in CPU condition codes without boxed value allocation.
- Snapshot tests in `tests/emitted_c.rs` reflect the new streamlined emitted C structure.
