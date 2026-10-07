# 28. Class and literal parity gaps found by issues 23-25

Status: resolved

Found while fixing issues 23-25; each diverges from node:

1. Both engines: a static field's computed key is evaluated before the class's method keys; node
   evaluates every class element's key in source order.
2. Both engines: a pair after a spread renames an already-named function: `{...{}, f: foo}` makes
   `foo.name` `"f"` (only anonymous function definitions take the property name).
3. Native: `F.prototype = undefined` does not stick, because `undefined` marks "prototype not yet
   created", so `class U extends F {}` succeeds where node throws.
4. Native: `extends boundFn` is refused reading `.prototype` of a bound function; node throws a
   `TypeError` (a bound function's `prototype` is `undefined`).

## Acceptance

- A regress case per item recorded from node; both engines match; `scripts/native.sh` 0 diverged.

## Comments

2026-09-27, branch `work/parity-28`.

**1. Class element key order (IR, both engines).** `strict_class_declaration` evaluated the
instance field keys first, then linked the heritage, then ran each static field's key *with its
value*, and only then evaluated and defined the methods -- so `static` fields and blocks also ran
before any method existed (`static x = C.m()` with `m` written below failed). Now one pass walks
the methods in source order and, before each computed method key, evaluates every field key
(instance or static) written before it (ordered by key span), converting with `ToPropertyKey` into
a hidden binding; a static field's key is refused as `prototype` there, which is where node
refuses it. Then the heritage is linked -- node 26 evaluates every element key *before* the
heritage check (`class U extends 1 { [k()]() {} }` calls `k`, then throws), so `link_heritage`
moved below the elements -- and only then do static field values and blocks run, in their order,
reading the stored keys.

**2. A pair after a spread renames its function (IR, both engines).** `DefineNamedProperty` now
carries `name_function: bool`; the IR sets it only for a method or a syntactically anonymous
function value. The interpreter and the native tail's generic path skip `SetFunctionName` when it
is false; every other consumer (graph validation/escape/analysis, the sunk and parallel paths,
direct, validation) matches `{ .. }` and never named. `identity.rs` hashes the flag.

**3. `F.prototype = undefined` (native runtime).** The closure's prototype slot used undefined for
"not created yet". A write of undefined (`properties.c`) now stores a hole, which
`kiln_c99_prototype_object` (`stores.c`) reads back as undefined instead of materialising an
object -- no new built-in, no struct field (a region closure's slot is the realm's). Consequences
handled: `new F()` substitutes `Object.prototype` for a primitive `prototype` in native (a `null`
one linked `null` before) and in the interpreter (`instantiate` linked the primitive), and native
`instanceof` against a primitive `prototype` answers false rather than matching an unrecorded
link.

**4. `extends boundFn` (native runtime, and the interpreter for a bound derived class).** A bound
function owns no `prototype`; `read.c` now answers the name from `getPrototypeOf(target)`: from
`Function.prototype`'s store or absent (`KILN_C99_ABSENT`, `kiln_c99_get_own_kind` row 25 -> 26)
for an ordinary target, the parent's `prototype` for a bound derived class. The interpreter's
bound read did the same for `prototype` only (it answered undefined for a bound derived class).

**Probes** (node 26.8.1): `conformance/regress/r42-class-element-key-order.js`,
`r43-spread-pair-keeps-function-name.js`, `r44-prototype-assigned-undefined.js`,
`r45-extends-bound-function.js`, and the same four under `conformance/native/` at
`native-release`. All match node in the interpreter, `native-dev` and `native-release`; the base
binary diverged on each (both engines on 1 and 2, native on 3 and 4).

**Found, not fixed** (pre-existing, left out of the probes):
- `native-release` names `{ n: (0, function () {}) }`'s function "n" (with or without a spread);
  the interpreter and `native-dev` answer "". Looks like the release graph path, not this change.
- `x instanceof F` with a primitive `F.prototype` is a `TypeError` in node; both engines answer
  false (native has no `thrown` in `kiln_c99_instance_of`).
- Native `Object.getOwnPropertyNames(f)` lists only the store (no `length`, `name`,
  `prototype`).
- `super()` into a bound constructor fails in both engines (`class X extends f.bind() {}` then
  `new X()`): interpreter "a function is not a constructor", native "cannot set a property on a
  null".
- Both engines model a bound function's [[Prototype]] as `Function.prototype` rather than its
  target's; only the `prototype` read follows the target now.

**Results** (branch release CLI, worktree `kiln-wt-28`). `mbx test -p kiln-ir -p kiln-graph -p
kiln-interpreter -p kiln-backend-c`: 69 + 34 + 200 + 206 + 36 passed, 0 failed.
`scripts/native.sh`: 296 matched, 1 refused (`shadow-realm`, as before), 0 diverged.
`kiln test conformance/regress --node`: 45/45 pass.
