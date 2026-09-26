# Issue 07: two emitter bugs the URL class exposed

Status: one fixed, one open
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
`.scratch/full-runtime/probes/url.mjs` natively.
