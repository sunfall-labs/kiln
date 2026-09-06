# A method carries a `prototype` property, and can be `new`ed

**Status:** resolved · **Severity:** medium — `new obj.method()` succeeds where every engine throws

    ({ m(){} }).m.hasOwnProperty("prototype")     // true, node false
    new ({ m(){} }).m()                            // succeeds, node TypeError

Same for a class method, a class static, and a getter or setter. A `prototype` a program can see
is also one it can feature-detect on, and `new` reaching a method is a call the language forbids.

## What is fixed and what is not

`has_prototype_property` answered "not an arrow". It now answers "not an arrow, and not async
unless it is an async generator", which fixes:

    (async function(){}).hasOwnProperty("prototype")        // now false, was true
    ({ async m(){} }).m.hasOwnProperty("prototype")         // now false, was true

An async generator deliberately keeps its `prototype`: that object is what its generator instances
inherit from, not something `new` reaches. Ordinary and generator functions keep theirs.

Methods are not fixed, and the reason is structural rather than an oversight.

## Why a method cannot be told apart today

Nothing records that a function was *written* as a method. `FunctionProto` carries `arrow`,
`generator`, `asynchronous` and `async_generator`, and none of them distinguishes

    ({ m(){} }).m          // no prototype
    ({ m: function(){} }).m // has one

which differ only in syntax. Two ways to carry it, both crossing the engine boundary:

* **A `FunctionProto` field.** Straightforward, but the proto is bytecode the C backend reads, so
  it means a format change, a `BYTECODE_VERSION` bump, and the C backend learning the field.
* **A runtime mark set by `Instruction::DefineMethod`,** which already exists and is exactly the
  instruction that knows. No bytecode change, but `Closure` has no interior mutability for it, and
  the C backend builds its own closures and needs the same mark.

Either is a "one table, two emitters" change (ADR 0043) and wants doing deliberately rather than
squeezed in beside an unrelated fix.

## Where it shows up

`language/statements/class` and `language/expressions/object/method-definition` both have rows
that turn on it, and a library that feature-detects with `typeof x.prototype` sees a method as
constructible.

## Resolution

- Added `pub is_method: bool` to `FunctionProto` in `kiln-ir` and bumped `BYTECODE_VERSION` to 3.
- In `kiln-ir::compile`: object methods, class methods, static methods, and accessors are marked with `is_method: true`. Class constructors explicitly have `is_method: false` (they are constructors, not methods).
- In `kiln-interpreter`: `closure_is_constructor` requires `!proto.is_method`, `has_prototype_property` requires `(!proto.is_method || proto.generator)`, and `has_property` (`in` operator) only returns true for `prototype` if `has_prototype_property(closure)`.
- In `kiln-backend-c`: `struct kiln_closure` stores `is_method`, `kiln_c99_construct` throws `TypeError` on methods, and `kiln_c99_get_own_kind` / `kiln_c99_own_property_present` only answer own prototype if `(!closure->is_method || closure->generator)`.
- Verified via `conformance/regress/r12-method-prototype.js` (matches Node and interpreter) and `conformance/native/method-prototype.ts` (interpreter and native parity).

