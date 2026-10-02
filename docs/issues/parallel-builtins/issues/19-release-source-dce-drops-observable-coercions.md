# 19. The release source DCE drops coercions and throwing calls it believes are pure

Status: resolved

Found by issue 18's `native-release` pass over `conformance/native`. A release build re-parses each
source module through `kiln_frontend_oxc::parse_dce` (`kiln-native`'s `dce_release_graph`), which
runs Oxc's `Compressor::dead_code_elimination` with `CompressOptions::dce()`. Oxc's side-effect
model is a bundler's, not the language's: it treats a relational comparison or `==` of any
operands as side-effect free, and a call of a known global function (`decodeURIComponent`) with
literal arguments as pure. So expression statements whose only effect is a coercion or a throw are
deleted, and a module variable that only such code writes is then constant-folded.

```js
let calls = 0;
const o = { valueOf() { calls += 1; return 2; } };
o < 10;                 // removed at release: `calls` is folded to 0
console.log(calls);     // interpreter and native-dev: 1; native-release: 0
```

Probes that diverge at `native-release` only (all match with the source DCE switched off, measured
with a throwaway build that skipped the `Compressor` call):

| probe | what is lost |
| --- | --- |
| `coercion-effects` | `value < 10;` in a loop (5 calls instead of 7) |
| `async-generator-coercion-loop` | the same inside a generator and an async function |
| `date-value-argument` | writes made by `[Symbol.toPrimitive]`/`valueOf` reached only through `new Date(obj)` (`hintSeen` stays `"none"`, `valueOfCalls` stays 0) |
| `text-escaping` | `try { decodeURIComponent("%"); } catch` -- the call is dropped, so nothing throws |

It also rewrote `const r = yield* inner(); yield r;` into `yield yield* inner()`, which exposed a
backend bug fixed on issue 18 (`c1d557ee`).

Pre-existing: the same four diverge with the compiler before issue 18. They were invisible because
these probes run at `native-dev`, and `native-dev` does not run the source DCE.

## Acceptance

- The source DCE removes only what the language says cannot be observed: an expression statement
  whose evaluation may call user code (ToPrimitive of a non-primitive operand, a getter, a Proxy
  trap) or may throw stays, and so do the writes it makes. Either configure Oxc (if 0.147 or later
  has the knob), or keep the DCE to unused declarations and bindings, or run kiln's own graph DCE
  (`kiln-graph`'s `eliminate_dead_code`, which already keeps `MAY_THROW | CALLS_USER_CODE`) instead.
- The four probes match at `native-release`; `scripts/native.sh` 0 diverged. The size win the DCE
  was bought for is measured before and after on the thirdparty bundles.

## Comments

2026-09-27, branch `work/release-bugs`.

**Root cause.** Oxc 0.147's side-effect model (`oxc_ecmascript::side_effects`) is a bundler's and
says so: "coercion methods are pure". Relational and `==` operators are pure whenever their
operands are, a template literal or `+` of an object/array *literal* is pure, `String`/`Date`/
`Boolean`/`Object`, `decodeURI*`/`encodeURI*`/`escape`/`isNaN`/`isFinite`/`parseInt`/`parseFloat`
and the `Math.*`-style methods are pure whatever their arguments are, and an unused object literal or
class is reduced to its computed keys' *expressions* and its heritage (dropping `ToPropertyKey` and
the constructor check). Oxc also marks a call it judges pure `/* @__PURE__ */` and then inlines
single-use `const`s across other code. That last step is how `date-value-argument` broke: `const
d1 = new Date(spy)` was inlined into `console.log(hintSeen, d1.getTime())`, *after* the read of
`hintSeen`. `TreeShakeOptions` has no knob for any of this.

**Fix** (`frontend:` commit, `kiln-frontend-oxc/src/dce_guard.rs`). The DCE is given an AST it
cannot misjudge. Before `Compressor::dead_code_elimination`, every expression whose evaluation may
run a coercion or a throwing built-in, and that is not proven to operate on primitives, is wrapped
in a call to the unresolvable global `kiln:observable(...)`. A call of an unknown global is an
effect in every Oxc judgment, so the wrapped expression is not removed, not inlined past other
code, and not folded into a decided branch. After the DCE the wrappers are removed. The name is
not a valid identifier, so a wrapper that somehow survived would be a loud `ReferenceError`, never
a wrong value (the unit test asserts none survive). What gets wrapped:
- `<`/`<=`/`>`/`>=`/`==`/`!=` and arithmetic/bitwise/unary `+ - ~` unless both operands are
  primitive; `x == null` is not wrapped (it converts nothing), and neither are `===`, `in` and
  `instanceof`, which Oxc already keeps unless they are provably safe;
- template literals with a non-primitive expression;
- calls/`new` of a global identifier or a property of one, unless annotated `@__PURE__` or on a
  small proven-pure list (`new Map()`/`Set`/`WeakMap`/`WeakSet`/`Object`/`Array`/`Error` with no
  arguments, `Symbol('lit')`, `Symbol.for('lit')`, `Object.create(null)`), plus
  `decodeURIComponent?.()`-style optional calls;
- object literals with a computed key that is not primitive or a well-known symbol (wrapped whole);
- classes whose heritage may not be a constructor, or with such a computed key. A wrapper would
  break the class's name inference, so the class gets a marker `static { kiln:observable(); }`
  block instead, which makes Oxc keep it whole. The block is removed afterwards.

"Primitive" is syntactic (literals, operators that always yield a primitive) or a never-reassigned
`let`/`const` initialised with one (via Oxc semantic), so `const LIMIT = 3; if (LIMIT > 5)` still
folds away.

**Tests.** `kiln-frontend-oxc`: `dce_keeps_expressions_whose_coercion_can_run_user_code`,
`dce_keeps_calls_and_classes_that_may_throw` (both fail with the guard switched off: the
comparison, the throwing call and the class key are deleted), and `dce_guard_keeps_what_the_pass_is_for`
(dead branches, unused functions, `new Map()`/`Symbol()` bindings and unused derived classes still
go). `kiln-native`: `release_source_dce_keeps_a_discarded_coercion_and_a_throwing_global_call`
(release build, node's answer). Probe `conformance/native/release-source-dce-observable-effects.js`
(`profile=native-release`): 19 discarded coercions, 6 throwing statements, and the `Date` reorder.
The four probes from the issue (`coercion-effects`, `async-generator-coercion-loop`,
`date-value-argument`, `text-escaping`) match at `native-release`.

**Size** (all 17 thirdparty bundles, `native-release`, base `e790de77` against this branch):
generated C +312 to +1,176 bytes per bundle (+7.6 KB total over ~290 MB), app objects +0.8 to
+2 KB, executables unchanged except `mustache` (+4 KiB, one page). This includes issue 21's
per-entry row helpers. The DCE's size win is intact.

**Filed while hunting the class** (engine bugs, every profile, not the DCE; their shapes are left
out of the probe): 23 (an array's `ToString` skips its elements' `toString`, in both engines), 24
(native computed keys skip `ToPropertyKey`'s user code), 25 (native `extends` accepts a
non-constructor).

**Not covered** (documented in the module): a read of a binding in its TDZ (Oxc ignores TDZ
errors), and a user-patched built-in that Oxc reads as a known pure global property (`Math.PI`
and similar). Both are rare in practice and neither is new.

**The release corpus pass** (every `conformance/native` probe forced to `native-release`, a copy
of `native.sh` with `timeout -k 10 900` per native run and no stop at the first broken build, on
the branch's final compiler): **279 matched, 0 diverged, 1 refused** -- `shadow-realm`, which
refuses in every profile. `scripts/native.sh`: 279 matched, 1 refused (`shadow-realm`), 0 diverged.
