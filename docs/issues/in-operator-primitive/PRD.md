# PRD: Refuse Primitive Right-Hand Side in `in` Operator

Status: resolved
Branch: `fix/in-operator-primitive`

## Problem Statement

Per ECMA-262 §13.10.1 (`RelationalExpression: PrivateIdentifier in ShiftExpression`), `in` requires the right-hand operand to be an Object. Kiln previously answered `false` for primitives (`"x" in 1`, `"x" in null`) rather than throwing `TypeError`.

## Resolution

- In interpreter: validated `is_an_object(right) || is_callable(right)`.
- In C backend: implemented accurate object reference predicate `kiln_c99_is_reference(v) || v.tag == KILN_VALUE_DATE` and added property checks for RegExp prototype accessors (`source`, `flags`, `lastIndex`, `exec`).
- Gained +4 Test262 rows in `language/expressions`.
