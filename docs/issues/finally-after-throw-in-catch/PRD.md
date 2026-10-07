# PRD: Finally Block Execution After Throw in Catch Block

Status: resolved
Branch: `fix/finally-after-throw-in-catch`

## Problem Statement

When a `catch` block threw an exception, its accompanying `finally` block was skipped rather than executed. In `kiln-ir` handler lowering, exception propagation from the catch block failed to link into the outer finally unwinder.

## Resolution

- In `crates/compile/kiln-ir/src/compile.rs`: linked catch block exception exit points to execute the enclosing finally block before rethrowing.
- Verified in both interpreter and C backend.
