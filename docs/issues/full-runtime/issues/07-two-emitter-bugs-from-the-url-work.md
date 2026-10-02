# Issue 07: two emitter bugs the URL class exposed

Status: closed (the second bug does not reproduce; workaround removed 2026-10-02)
Feature: full-runtime

**Fixed** (`emit_graph/ops/call_method.rs`, 2026-09-19): a devirtualized method call guarded on
`obj->shape == <cached shape>` where the cached static starts NULL; a receiver in dictionary
mode has a NULL shape too, so the guard matched with a NULL row and hit
`__builtin_unreachable()`. `class K { get href() { return this._components()[0]; } }` crashed
every release build (exit 139) and the class instances here *are* in dictionary mode after the
constructor's writes -- which is its own question: why a fresh class instance leaves shape mode.

**Open**: the bootstrap module emitted with `generic_only` reads `undefined` back from an
object literal that holds a call:

```ts
this[STATE] = { c: parseRecord(input, base), sp: undefined, input: undefined, base: undefined };
// this[STATE].c is undefined; hoisting the call into `const rec` first is fine
```

The same class as a user program under `native-release-size` (also generic-only) is correct,
so the trigger is something about the bootstrap module -- its size, its `compact` emission, or
the intrinsic being a `Direct` method value. `crates/semantics/kiln-builtins/src/url.ts` keeps the hoist
with a comment pointing here; the repro is to fold it back and run
`docs/issues/full-runtime/probes/url.mjs` natively.

## Comments

**2026-10-02.** The open half does not reproduce. With the hoist folded back into `url.ts`, native
output matches the interpreter at HEAD, under `KILN_GC_STRESS=1`, and at `d0b176d6` (where the hoist
was introduced) under `native-dev`, `native-release-size`, `native-release` and `--node`. The
bootstrap really is emitted generic-only (`kiln-native/src/graph.rs`, `bootstrap_policy.generic_only`),
so the path was exercised. Reverting `f92de8ec` (the cached-literal write barrier) did not bring it
back either, so the cause is unknown and was probably a transient state of the tree. The hoist and
its comment are gone, and `conformance/native/object-literal-call-value.mjs` holds the shape --
a call inside a literal, beside the hoisted form, under allocation churn, plus the real `URL`
class -- so a recurrence fails the native corpus by name.
