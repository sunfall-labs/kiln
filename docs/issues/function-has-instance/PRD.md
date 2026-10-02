# PRD: Function.prototype[Symbol.hasInstance] Builtin Property

Status: resolved
Branch: `fix/function-has-instance`

## Problem Statement

`Object.getOwnPropertyDescriptor(Function.prototype, Symbol.hasInstance)` answered `undefined` in Kiln, where ECMA-262 specifies a non-writable, non-enumerable, non-configurable (`w0e0c0`) built-in method.

## Resolution

- Published `[Symbol.hasInstance]` through `tabulated_function_property` with attributes `w0e0c0`.
- Set `native_js_name` to `"[Symbol.hasInstance]"` and `native_length` to 1.
- Split `ordinary_has_instance` from `instance_of_value` so the method performs the prototype chain walk without self-recursion.
- Added 14 Test262 rows in `built-ins/Function`.
