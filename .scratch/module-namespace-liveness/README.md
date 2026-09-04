# A module namespace is a snapshot; the bindings beside it are live

**Status:** open. **72 test262 rows**, all in `language/expressions/dynamic-import/usage`, plus
the static-import equivalents.

## What is wrong

Named imports track the exporting module. The namespace object does not.

```js
// mod.mjs
export let counter = 1;
export function bump() { counter = counter + 1; }

// main.mjs
import * as ns from "./mod.mjs";
import { counter, bump } from "./mod.mjs";
bump();
// node:  ns.counter 2, counter 2
// kiln:  ns.counter 1, counter 2
```

Every other engine treats a namespace as an exotic object whose `[[Get]]` reads the binding.
Kiln builds an ordinary frozen object and copies the values in at link time, so it reports
whatever was true when the link ran.

The 72 rows read as `Expected SameValue(«export default function fn() {…»)` because the fixtures
update their default export after the import resolves and then compare.

## What this is *not*

Filed for most of this session as "dynamic-import needs a referrer-aware `ModuleLoader`". That was
wrong, and wrong for an instructive reason: the probe that produced it went through `kiln run`,
which uses a different loader than the conformance runner. The runner records a base directory per
case (`kiln-conformance/src/module_loader.rs`) and resolves these fine. The 114 rows under
`dynamic-import` were 72 of *this* and 42 of a runner policy that has since been fixed.

## The shape a fix probably takes

The machinery is half there. `refresh_self_imports` already keeps a namespace in step for a module
that imports **from itself**, walking `self_namespaces` and re-reading each slot -- but that works
because those slots are the same module's locals.

The lead worth following: named imports across modules *are* live, which means the exporting
module's bindings are shared as **cells**. So a cross-module namespace could hold those cells
rather than copies, and a read of a namespace property could deref one. `ObjectData` already knows
which objects are namespaces (`is_module_namespace`), which is where such a read would branch.

What has to be got right, and is why this is not a small change: a binding still in its temporal
dead zone must read as a ReferenceError rather than `undefined` (`refresh_self_imports` says so
explicitly), circular imports must not observe a half-built namespace, and an async module's
bindings settle after its body suspends.


## Tested 2026-09-03: the cells lead does not close it on its own

The namespace builder derefs at construction -- `data.define_value(&name, read_through_cell(&value), ..)`
-- and `read_through_cell` is called from that one place. So "store the cell instead" is a
one-character change, and it is not enough.

`ObjectData`'s readers hand back *references* (`get_own_property` -> `Option<&Property>`), and
there are 51 `get_own` call sites. Nothing on that path derefs a cell, so a namespace holding one
would answer `Value::Cell` to `Object.keys`, `getOwnPropertyDescriptor`, spread, `for...in` and
`JSON.stringify` -- a silent wrong answer of exactly the kind this backend refuses elsewhere, in
exchange for fixing the plain read.

The two shapes that would actually work:

- **Deref at a chokepoint.** Give `ObjectData` a value-returning read that derefs when
  `module_namespace` is set, and route the paths that can see a namespace through it. The work is
  in finding those paths, not in the deref.
- **Refresh on write**, the way `refresh_self_imports` does for a self-import -- but that walks
  slots at a known moment, and a cross-module namespace has no such moment: it would need a hook
  on writes to an exported binding, which cells do not have.

So the row count stands and the difficulty does too. This is the third claim on this branch to be
tested rather than asserted, and the second to survive.
