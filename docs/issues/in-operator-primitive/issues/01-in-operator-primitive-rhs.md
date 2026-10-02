# `in` Operator Accepts Primitive RHS Instead of Throwing TypeError

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-03 (+4 rows in `language/expressions`).
Initial regression caught by `scripts/thirdparty.sh`: `kiln_c99_is_reference` was narrower than general object identity and refused Date objects. Fixed by handling Date tag explicitly.

## Symptoms

```js
"x" in 1;    // Expected: TypeError; Was: false
"x" in null; // Expected: TypeError; Was: false
```
