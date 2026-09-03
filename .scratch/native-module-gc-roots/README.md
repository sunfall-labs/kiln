# A module with module-scope globals cannot be compiled natively

**Status:** open, pre-existing. Reproduced at `52e60b1f` in a clean worktree, so it predates
the 2026-09-03 conversion work on this branch.

## Repro

```js
// gc1.js
function t(label, fn) { try { fn(); console.log(label, "ok"); } catch (e) { console.log(label, e.constructor.name); } }
class B {}
var x = 1;
var y = 2;
t("a", function () { return new (class extends B { constructor(){ super(); } })(); });
console.log(x, y);
```

```
kiln --one-shot run --native gc1.js
kiln: native toolchain failed: C compilation failed:
  error: use of undeclared identifier 'KILN_GC_PHASE_MARK'
  error: call to undeclared function 'kiln_c99_gc_mark_values'
  error: use of undeclared identifier 'kiln_gc_heap'
```

The interpreter runs it. `kiln run --native` on a trivial file works, so the native backend is
not broken generally.

## Cause

`render_gc_roots` in `crates/kiln-backend-c/src/lib.rs` emits, for a module-scope unit:

```c
if (phase == KILN_GC_PHASE_MARK) kiln_c99_gc_mark_values((kiln_gc_heap *)heap, <sym>_globals, N);
```

All three names it uses live in `prelude_runtime.c`:

| name | defined at |
| --- | --- |
| `KILN_GC_PHASE_MARK` | `prelude_runtime.c:924` (`#define`) |
| `kiln_gc_heap` | `prelude_runtime.c:1058` (typedef) |
| `kiln_c99_gc_mark_values` | `prelude_runtime.c:2017` (**`static`**) |

The emitted module unit includes `prelude_declarations.h` and `prelude_core.c`, **not**
`prelude_runtime.c`. The line immediately below it calls `kiln_c99_gc_unit_roots`, which is also
`static` but lives in `prelude_core.c` -- so it resolves, and that is why only this one line
fails.

The neighbouring `keyed` branch of the same function works for the same reason: it uses
`KILN_GC_PHASE_RESET`, which `prelude_declarations.h:6` does declare. `KILN_GC_PHASE_MARK` is
not declared there.

## Why this is not a one-line fix

Adding the three names to `prelude_declarations.h` is not enough: `kiln_c99_gc_mark_values` is
`static` in a translation unit the module does not include, so a declaration would compile and
then fail to link. A fix has to choose one of:

- export a marking entry point from `prelude_core.c`, which the module unit already includes --
  most likely as an argument to the `kiln_c99_gc_unit_roots` call already emitted on the next
  line, so the module hands its globals to a hook that is already resolving; or
- include `prelude_runtime.c` in module-scope units, which is a much larger change to the unit
  assembly.

The first is the smaller one and keeps the "one place knows the sections" property.

## Why the gates did not catch it

`scripts/native.sh` and `scripts/thirdparty.sh` are both green (91 matched / 6 refused / 0
diverged; 19 matched / 0 refused / 0 diverged). Whatever path those take does not produce a
module-scope unit with globals -- worth confirming, because a corpus of 19 real library bundles
not reaching this line is itself surprising and may mean the bundles are compiled as a single
whole-program unit where the prelude sections are all present.

That is the second thing to check: if so, `conformance/native` wants a probe that is a *module*
with module-scope `var`s, because nothing there covers this shape today.
