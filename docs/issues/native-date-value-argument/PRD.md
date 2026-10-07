# PRD: Native Date Constructor Object Coercion via ToPrimitive

Status: resolved
Branch: `fix/native-date-value-argument`

## Problem Statement

In the C native backend, `new Date(obj)` converted single arguments with a pure numeric conversion rather than calling `ToPrimitive(obj, "default")`. An object without numeric value yielded `NaN` / `Invalid Date` without invoking `[Symbol.toPrimitive]`.

## Resolution

- In native prelude C: routed single-argument non-string/non-date constructors through `kiln_c99_to_primitive(..., "default")`.
- Added native conformance probe `conformance/native/date-value-argument.ts`.
