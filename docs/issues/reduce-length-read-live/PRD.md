# PRD: Array.prototype.reduce Reads Array Length Once

Status: resolved
Branch: `fix/reduce-length-read-once`

## Problem Statement

ECMA-262 §23.1.3.24 dictates that `Array.prototype.reduce` captures `len = LengthOfArrayLike(O)` once at start of iteration. Kiln's dense array fast paths previously re-read the array length on each iteration, causing elements appended by the callback to be improperly visited.

## Resolution

- In interpreter and native C backend: captured array length once before iterating callback loop.
- Added tests verifying that callback-appended elements are skipped.
