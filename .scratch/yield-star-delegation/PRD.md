# PRD: Specification-Compliant `yield*` Iterator Delegation Protocol

Status: resolved
Branch: `feat/yield-star-delegation`

## Problem Statement

`yield*` (`crates/compile/kiln-ir/src/compile.rs:3257`, `yield_delegate`) currently lowers to a simple loop invoking `iterator.next()`. ECMA-262 §14.4.14 (`YieldExpression: yield * AssignmentExpression`) defines a three-way delegation protocol forwarding `.return()` and `.throw()` completions from the outer generator to the inner iterator.

## Impact

Costs ~53 Test262 rows across `language/expressions/yield`:
- 13 rows in `star-rhs-iter-rtrn-*`
- 9 rows in `star-rhs-iter-thrw-*`
- 5 rows in other delegation edge cases

## Architecture Plan

1. **IR Instruction / Lowering Protocol**:
   Ensure both interpreter and C native backends support delegating `return` and `throw` resumptions:
   - Handle inner `return()` when outer generator is closed via `g.return(val)`.
   - Handle inner `throw()` when outer generator is thrown into via `g.throw(err)`.
   - If inner iterator has no `.throw()`, close inner iterator via `.return()` and throw `TypeError`.
2. **Interpreter & C Backend Parity**:
   Implement consistent resumption handling per ADR 0043.
3. **Verification**:
   Pass the 53 Test262 rows in `language/expressions/yield`.
