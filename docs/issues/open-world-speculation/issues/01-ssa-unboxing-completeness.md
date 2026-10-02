Status: closed
Feature: open-world-speculation

# 01 - SSA Unboxing Completeness for Math & Bitwise Operations

## Goals
1. Extend `is_math_intrinsic` in `crates/native/kiln-backend-c/src/emit_graph/body.rs` to cover all 26 pure mathematical functions on `Math` (1-arg: `sqrt`, `abs`, `floor`, `ceil`, `round`, `trunc`, `sign`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `exp`, `expm1`, `log`, `log2`, `log10`, `log1p`, `cbrt`; 2-arg: `pow`, `min`, `max`, `atan2`, `hypot`), plus `clz32`, `fround`, and `imul`.
2. Extend `clones.rs` to allow all pure math functions in restartable fast clones.
3. Allow bitwise operations (`&`, `|`, `^`, `<<`, `>>`, `>>>`) between integer operands (`ints.contains(left) && ints.contains(right)`) and `UnaryOp::BitwiseNot` in `body.rs` so that integer bitwise math does not box operands as `Value`.

## Acceptance Criteria
- Full test pass in `kiln-backend-c`. (Passed: 160 unit tests, 36 snapshot tests)
- Native parity suite matches across interpreter and native (`scripts/native.sh`). (Passed: 206 matched, 0 diverged)
- Executable test added: `executable_math_all_pure_functions_unboxed`.
