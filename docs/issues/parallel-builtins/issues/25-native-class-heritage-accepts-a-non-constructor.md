# 25. The native backend accepts a class heritage that is not a constructor

Status: resolved

Found while writing issue 19's probe. `class U extends n {}` with `n` neither a constructor nor
`null` is a `TypeError` when the class is evaluated (ClassDefinitionEvaluation step 8). The native
backend evaluates the class without throwing; the interpreter throws. Every native profile.

```js
function h() {
  try { const n = 1; class U extends n {} return "no throw"; }
  catch (e) { return e.constructor.name; }
}
console.log(h());
// interpreter and node: TypeError
// native (dev and release): no throw
```

## Acceptance

- `extends` a number, a string, an arrow function, an async function, a generator function and an
  object whose `prototype` is neither an object nor `null` all throw `TypeError` natively; a probe
  holds them.

## Comments

2026-09-27, branch `work/parity-23-25`.

**Root cause.** The class lowering read `superclass.prototype`, linked `C.prototype` to it, then
linked `C` to the superclass with `SetPrototype`. The only heritage check was in the interpreter's
`SetPrototype` (a function target and a non-constructor), after `prototype` had been read; the
native `SetPrototype` had none, and linking to a primitive is silently ignored (the
`{ __proto__: 1 }` rule), so natively every heritage defined a class. Both engines also skipped
step 8.b's check that `prototype` is an object or `null` (`F.prototype = 3; class U extends F {}`
defined a class), and both threw for `extends null` by reading `null.prototype`.

**Fix** (`ir:`/`backend-c:` commit). `link_heritage` (`kiln-ir/compile/classes.rs`) emits
ClassDefinitionEvaluation's steps in order: `null` links `C.prototype` to `null` and leaves `C` on
`Function.prototype`; otherwise `SetPrototype(C, superclass)` first -- the constructor check --
then `superclass.prototype` is read and must be `typeof` "object" or "function", else a
`TypeError`. Natively the tail's `SetPrototype` calls `kiln_c99_set_prototype_checked`
(`runtime/calls.c`, an existing emission path, not a built-in): a function target with a heritage
that is neither `null` nor a constructor throws through the unwind. The constructor predicate is
the one `kiln_c99_construct` applies to a generated function (not an arrow, generator, async
function or method), followed through a bound function and a proxy; a built-in that is not a
constructor has no `prototype`, which the IR check refuses.

**Probes.** `conformance/regress/r41-class-heritage-checks.js` (node 26.8.1): `extends` a number, a
string, `undefined`, a symbol, a plain object, an arrow (with and without an assigned
`prototype`), an async function, a generator, an async generator, a method, constructors whose
`prototype` is `3` or a string, a class expression -- all `TypeError`; a function, a class, a
built-in, `prototype = null` and `prototype = function` define; `extends null`'s two links; a
derived `Array`. Also `conformance/native/class-heritage-checks.js` at `native-release`. Matches
node in the interpreter, `native-dev` and `native-release`; the base binary diverged in all three.

**Found, not fixed** (native, pre-existing, left out of the probe): `F.prototype = undefined` does
not stick -- `undefined` is the lazy-prototype sentinel in `kiln_c99_prototype_object`, so the read
materialises a fresh object and `class U extends F {}` defines where node throws; and
`extends boundFn` refuses at `GetProperty(prototype)` ("a member of a bound function other than
name and length") where node throws a `TypeError` (a bound function has no `prototype`).

**Results** (branch's release CLI, worktree `kiln-wt-parity3`). `mbx test -p kiln-ir -p
kiln-interpreter -p kiln-backend-c`: 69 + 200 + 203 + 36 passed, 0 failed (the one change to a
snapshot is issue 25's ABI surface). `scripts/native.sh`: 286 matched, 1 refused (`shadow-realm`,
as before), 0 diverged. `kiln test conformance/regress --node`: 41/41 pass.
