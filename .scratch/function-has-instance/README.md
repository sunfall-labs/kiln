# `Function.prototype[@@hasInstance]` is missing, and the obvious fix regresses badly

**Status:** open. Attempted 2026-09-03, reverted. ~6-12 test262 rows available, but the naive
route costs far more than it wins.

## The gap

`Object.getOwnPropertyDescriptor(Function.prototype, Symbol.hasInstance)` answers `undefined`.
Node answers `w0e0c0` -- a method that is neither writable nor configurable.

`instanceof` itself is already right: `instance_of_value` reads `@@hasInstance` off the
right-hand side and falls back to `OrdinaryHasInstance` when it is absent, so the operator
behaves correctly. What is missing is the *property*, and two things follow from its absence:

- a program cannot read or call `Function.prototype[Symbol.hasInstance]`;
- assigning over it is accepted. Because the inherited property is non-writable,
  `C[Symbol.hasInstance] = f` on a function must be a `TypeError` in strict code. Kiln takes the
  assignment, so a class can silently redefine `instanceof` where the language refuses.

## What was tried, and what it broke

Publishing it the way every other `Function.prototype` member is published:

- a `Native::FunctionHasInstance` variant, dispatched on the machine because
  `OrdinaryHasInstance` walks a prototype chain (this part is right, and required splitting
  `ordinary_has_instance` out of `instance_of_value` so the method does not find itself and
  recurse for ever);
- `"@@hasInstance"` added to `prototype_accessor_names(PrototypeTable::Function)`.

The second is the mistake. `"@@hasInstance"` is a *reserved key* -- the internal spelling that
`symbol::is_reserved` filters out of every enumeration -- and putting it in a name list leaks it
into general property listing. The gate caught it immediately and it was not close:

| gate | before | after |
| --- | --- | --- |
| native parity | 91 matched, 0 diverged | 80 matched, **11 diverged** |
| thirdparty | 19 matched, 0 diverged | 18 matched, **1 diverged** |
| node corpus | 35 pass | **34 pass, 1 fail** |
| `built-ins/Object` | 6565 | **5727** (-838) |
| `language/expressions` | 20290 | **19904** (-386) |

Reverted whole; the numbers above were confirmed restored.

## What a real fix needs

The symbol-keyed member has to be published through whatever path *symbol* keys take, not the
string-name list -- the same distinction `own_property_keys` already makes when it filters
reserved keys out. It also needs two things the string path cannot express:

- attributes of `w0e0c0`, where every other prototype method is `w1e0c1`;
- the function's `name`, which is `"[Symbol.hasInstance]"` and not `"hasInstance"`.

Worth doing only alongside a look at how symbol-keyed built-in members are meant to be published
generally, because `@@toPrimitive`, `@@toStringTag` and `@@iterator` on the intrinsics raise the
same question.
