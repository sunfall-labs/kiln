# Ideal Codegen Architecture for Kiln

Status: completed
Branch: `perf/ideal-codegen-architecture`
Owner: Antigravity, 2026-09-03

## Context & Motivation

Kiln compiles JavaScript and TypeScript to native executables by emitting C99 code (`kiln-backend-c`) and linking it against a Tokio/Hyper runtime.

An analysis of Robin Freyler's [Wasmi 2.0](https://wasmi-labs.github.io/blog/posts/wasmi-v2.0/) demonstrates that several fundamental engine bottlenecks are present in Kiln's emitted C code:
1. **Simulated Stack Machine vs Virtual Registers**: Kiln lowers stack bytecode into literal C stack-array operations (`kiln_value stack[max_stack]`, `stack[sp++]`, `stack[--sp]`). This causes unnecessary memory traffic and dynamic stack-pointer indexing. When pointers escape (such as during host calls or runtime helper invocations), downstream C compilers (Clang/GCC) fail to promote the stack array to registers.
2. **Calling Convention Over-subscription**: Internal function signatures (`kiln_body_fn`) take 9 arguments, including `receiver` passed by value as a 16-byte struct (`kiln_value`). This exceeds the 6 general-purpose registers (GPRs) on SysV AMD64 and 8 GPRs on AArch64, spilling 4 arguments onto the stack on x86_64 on every function call.
3. **Compare-and-Branch Boxing**: Relational comparisons (`<`, `<=`, `>`, `>=`) construct a boxed `kiln_value` boolean, push it to the stack, pop it in `JumpIfFalse`/`JumpIfTruthy`, and call `!kiln_c99_truthy(...)`.
4. **Dynamic Closure Dispatch for Internal Calls**: Local functions in the same module are compiled into closure heap/stack objects and dispatched indirectly via function pointers instead of direct static C calls.
5. **Lack of Branch Predictor Annotations**: Hot type checks and fast-path numeric checks lack `__builtin_expect` / `[[likely]]` annotations, leading to cold bailout code polluting hot instruction cache lines.

## Goals

1. **Eliminate Argument Spills in the Calling Convention**: Refactor `kiln_body_fn` to pass an execution context (`kiln_call_context *ctx`) and pass `receiver` by pointer, reducing argument registers to <= 6 GPRs on SysV AMD64 and AArch64.
2. **Direct Compare-and-Branch Control Flow**: Fuse comparison instructions with conditional jumps to directly branch (`if (!(a < b)) goto label;`) without intermediate boolean allocation, stack push/pop, or dynamic truthiness inspection.
3. **Fast-Path Branch Prediction**: Introduce `KILN_LIKELY` and `KILN_UNLIKELY` annotations on hot type checks and error paths.
4. **Direct Static Calls for Internal Functions**: Emit direct C function invocations for statically known local functions.
5. **Expression Temporaries (SSA / Virtual Registers)**: Replace intra-block `stack[sp++]` / `stack[--sp]` traffic with C local variables.

## Architecture

### 1. Zero-Spill Calling Convention

```c
typedef struct kiln_call_context {
    kiln_host *host;
    kiln_value *thrown;
    struct kiln_generator *frame;
    int depth;
} kiln_call_context;

typedef int (*kiln_body_fn)(
    kiln_call_context *ctx,
    kiln_value *result,
    kiln_value *upvalues,
    const kiln_value *receiver,
    const kiln_value *args,
    size_t argc
);
```

- **SysV AMD64 register allocation**:
  - `ctx`: `rdi`
  - `result`: `rsi`
  - `upvalues`: `rdx`
  - `receiver`: `rcx`
  - `args`: `r8`
  - `argc`: `r9`
  All 6 arguments fit in GPRs. **Zero stack spills.**

### 2. Compare-and-Branch Fusion

When `Instruction::Binary(op)` is a comparison and is followed by `Instruction::JumpIfFalse(target)` (or `JumpIfTruthy`):
```c
if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER)) {
    if (!(a < b)) goto kiln_label_target;
} else {
    answer = kiln_c99_binary(operator, left, right);
    if (!kiln_c99_truthy(answer)) goto kiln_label_target;
}
```

### 3. Branch Predictor Hints

In `prelude_declarations.h`:
```c
#if defined(__GNUC__) || defined(__clang__)
#define KILN_LIKELY(x) __builtin_expect(!!(x), 1)
#define KILN_UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define KILN_LIKELY(x) (x)
#define KILN_UNLIKELY(x) (x)
#endif
```

## Plan of Record

- [x] **Milestone 1**: Implement `KILN_LIKELY` / `KILN_UNLIKELY` annotations in the prelude and emit on numeric fast paths.
- [x] **Milestone 2**: Implement comparison + conditional jump fusion in `kiln-backend-c`.
- [x] **Milestone 3**: Overhaul `kiln_body_fn` calling convention with `kiln_call_context` across `prelude_types.h`, `prelude_declarations.h`, `prelude_core.c`, and `lib.rs` (ADR 0052: zero argument spills on SysV AMD64 & AArch64).
- [x] **Milestone 4**: Zero-copy call argument passing: eliminated temporary argument array copying and heap allocations in `kiln_c99_op_call` and `kiln_c99_op_call_method`.
- [x] **Milestone 5**: Stack traffic reduction via instruction fusion: fused `Const` + `StoreLocal`, `LoadLocal` + `StoreLocal`, and `Binary` + `StoreLocal` into direct register/local writes, eliminating stack push/pop churn in loops.
