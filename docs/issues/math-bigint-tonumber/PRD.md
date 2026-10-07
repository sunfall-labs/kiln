# PRD: Math Intrinsics Reject BigInt with TypeError

Status: resolved
Branch: `fix/math-bigint-tonumber`

## Problem Statement

`Math.sqrt(10n)` and other `Math` builtins answered numbers instead of throwing `TypeError`. Per ECMA-262 §21.2, `Math` methods apply `ToNumber` to arguments, which explicitly rejects BigInt values with a `TypeError`.

## Resolution

- Updated `ToNumber` conversion in shared math builtins to enforce BigInt rejection.
- Verified against Node and interpreter test suites.
