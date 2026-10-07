# PRD: Per-Evaluation Brand Check for Private Class Members

Status: resolved
Branch: `fix/private-names-per-evaluation`

## Problem Statement

Every evaluation of a class expression must mint unique private names. Kiln previously keyed private members using static compile-time scope IDs (`@@private:{scope}:{name}`), allowing two instances from different evaluations of the same class factory to access each other's private members.

## Resolution

- In IR and codegen: class evaluation mints a brand object (`Value::Array` or unique heap pointer) stored in a hidden binding.
- `GetPrivate` and `SetPrivate` take the brand as an operand and verify that the instance's installed brand matches the accessing class brand.
- Gained +53 Test262 rows in `language/expressions/class` and `language/statements/class`.
