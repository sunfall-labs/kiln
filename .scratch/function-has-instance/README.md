# `Function.prototype[@@hasInstance]` is missing, and the obvious fix regresses badly

**Status:** fixed 2026-09-03. **This filing's diagnosis was wrong**, and the correction is the
useful part of it -- see the last section. +14 rows in `built-ins/Function`.

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


## 2026-09-03, second attempt: the diagnosis above is wrong

The name list is not what cost 838 rows. Publishing the property at all did, and it did so through
a mechanism this filing never suspected.

Isolated by building both halves separately and measuring each: with the member published and the
name list untouched, `built-ins/Object` was **still** 5777 (-838) and `language/expressions` still
20087 (-397) -- the same numbers, to the row, that were blamed on the enumeration leak. So the leak
was never the cause. `Object.getOwnPropertyNames(Function.prototype)` was checked directly and does
not contain the reserved key.

The actual cause is one argument. `instance_of_value` carried an `error_prototype: Option<&Value>`
supplied by the *operator*, and `Value::Native(Native::Error(_), _)` answered `false` when it was
`None`. `Function.prototype[@@hasInstance]` is reached with no operator context, so it passed
`None`, so `e instanceof TypeError` became false for every error in the realm. test262's own
`assert.throws` is written with `instanceof`, so every test that asserts a throw -- hundreds of
them, across chunks with nothing to do with `instanceof` -- failed at the harness.

The fix is to stop threading it: the kind is in the constructor and the realm can always answer
`error_prototype(kind)` itself. A caller that cannot supply a piece of context is a sign the
context did not belong in the signature.

## What it took, once the cause was known

Exactly what the section above predicted, minus the part that was wrong:

- the member is published through `tabulated_function_property`, which is the existence-and-value
  path -- the same one `Date.prototype[@@toPrimitive]` uses. The enumeration list is untouched.
- `w0e0c0` in `intrinsic_own_attributes`: the one built-in the specification locks on both axes.
- `native_js_name` gives `[Symbol.hasInstance]`; `native_length` gives 1.
- `ordinary_has_instance` split out of `instance_of_value`, so the method performs the walk rather
  than the operator -- otherwise it reads itself back off the receiver and recurs for ever.
- the callable refusal stays in the *operator*: `OrdinaryHasInstance` answers `false` for a
  non-callable, and only `InstanceofOperator` throws. Putting it in the shared half made
  `Function.prototype[@@hasInstance].call({}, 1)` throw where node answers false.

## Still open, and now visible

`getOwnPropertySymbols` on an intrinsic answers `[]` where node lists 1-2. The symbol members are
readable and have correct descriptors; they are simply absent from the key listing, which is
another instance of the same shape -- existence and enumeration are answered by different tables.
