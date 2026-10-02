# PRD: Invalidate Prototype Method Cache on Prototype Write

Status: resolved
Branch: `fix/method-cache-prototype-write`

## Problem Statement

In `kiln_c99_val_call_method`, resolving a prototype method remembered the closure by value keyed only on the receiver's shape. If the prototype's property was subsequently reassigned or deleted, the call site continued calling the stale cached method closure.

## Resolution

- Linked prototype version / mutation counter to inline cache entries in C runtime.
- Added invalidation check on prototype writes so calls observe new method values.
