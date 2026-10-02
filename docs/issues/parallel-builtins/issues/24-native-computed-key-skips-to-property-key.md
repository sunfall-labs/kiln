# 24. The native backend's computed key does not call `ToPropertyKey`'s user code

Status: resolved

Found while writing issue 19's probe. A computed key in an object literal is converted with
`ToPropertyKey`, which for an object calls its `toString` (hint "string"). The native backend keys
the property `"[object Object]"` and calls nothing; the interpreter is right. Every native profile.

```js
const seen = [];
const k = { toString() { seen.push("k"); return "kk"; } };
function g() { const o = { [k]: 1 }; return Object.keys(o); }
console.log(g().join(), seen.join());
// interpreter and node: kk k
// native (dev and release): [object Object]
```

Check class member computed keys (`class C { [k]() {} }`) at the same time.

## Acceptance

- The repro and its class-member form match at every native profile; a probe holds both, plus a
  key whose `toString` throws and a `Symbol.toPrimitive` key.

## Comments

2026-09-27, branch `work/parity-23-25`.

**Root cause.** No `ToPropertyKey` was emitted for a computed key in an object literal or a class
method; the key was converted by whatever defined it. The interpreter's `NewObject`,
`DefineNamedProperty`, `DefineMethod` and `DefineAccessor` convert with `computed_key` -- after
the value has run -- and the native defines use `kiln_c99_property_key`, which names every object
"[object Object]" and calls nothing. So the interpreter was right about the key and wrong about
the order (`{ [k]: v() }` ran `v` before `k`'s `toString`, and every later value too), and native
was wrong about both.

**Fix** (`ir:` commit), at the one owner both engines share: the IR emits `ToPropertyKey` where the
key is written, between key and value, so both engines and every native emission path (tail,
graph, compact helpers) receive a primitive key. With the object beneath the key (a pair after a
spread or a named function value, an accessor, a class method or accessor, a static field -- which
already had it through `guard_static_prototype_key`) that is the instruction as it is. A pair in
a pending run has no object beneath it -- only the previous value, which may be nullish and would
make `ToPropertyKey` skip -- so `pending_literal_key` duplicates the key as its own stand-in (a
nullish key is a primitive and needs no conversion) and carries the converted key past it through
one hidden local per literal. A key that is syntactically a literal or a template emits nothing
new.

**Probes.** `conformance/regress/r40-computed-key-to-property-key.js` (node 26.8.1): the repro and
its class-method form; `@@toPrimitive` keys on a pair, a method and a getter; a `valueOf`-only key;
a throwing key in a literal, a class method and a static field; the key/value order across pairs,
a spread, a named-function pair and an accessor; class-method key order; a symbol from
`@@toPrimitive`; function names after the converted key. Also
`conformance/native/computed-key-to-property-key.js` at `native-release`. Matches node in the
interpreter, `native-dev` and `native-release`; the base binary diverged in all three (native at
the repro, interpreter at the order).

**Found, not fixed** (both engines, pre-existing): a static field's computed key is evaluated before
the class's method keys, where node evaluates every element key in source order
(`class C { [a]() {} static [b] = 1 }` runs `b` first); and a pair after a spread names its function
value after the key even when the function already has a name (`{ ...{}, f: foo }` renames `foo`
to `"f"`), because `DefineNamedProperty` names unconditionally and the post-spread path uses it
for every pair.

**Results** (branch's release CLI, worktree `kiln-wt-parity3`). `mbx test -p kiln-ir -p
kiln-interpreter -p kiln-backend-c`: 69 + 200 + 203 + 36 passed, 0 failed (the one change to a
snapshot is issue 25's ABI surface). `scripts/native.sh`: 286 matched, 1 refused (`shadow-realm`,
as before), 0 diverged. `kiln test conformance/regress --node`: 41/41 pass.
