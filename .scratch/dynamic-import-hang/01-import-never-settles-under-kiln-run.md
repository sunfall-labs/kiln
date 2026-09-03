# `import()` never settles under `kiln run`

**Status:** open · **Severity:** high — silent, no diagnostic, no exit code

## Repro

    // dep.mjs
    export let x = 1;

    // main.mjs
    console.log("A");
    import("./dep.mjs").then(m => console.log("then", m.x), e => console.log("rej", e));
    console.log("B");

    $ kiln run main.mjs
    A
    B                      # node also prints `then 1`; exit status 0 either way

Neither handler runs. The same holds for `await import(...)` at top level and for `await`
inside an async function -- the module simply stops at the await with no output, no error and
exit 0. It happens whether or not the target was also imported statically, so it is not about
discovering a module late.

## Why it matters more than the row count suggests

test262's `language/expressions/dynamic-import` is 1430/174, so the corpus harness resolves these
fine -- this is the CLI's own module path. A promise that never settles is the worst shape a
failure can take: the program does not stop, does not throw, and does not report anything, so a
caller waits forever and a script exits successfully having done half its work.

## Not investigated

Found while probing module live bindings; the interpreter changes in this session do not touch the
dynamic-import path, but that was reasoned rather than bisected. Worth confirming against an
earlier build before assuming it is long-standing.

## Related, found in the same probe

Namespace objects are not live. A module's exported binding updated after import is visible
through the direct import and not through the namespace:

    import { x, bump } from "./dep.mjs";
    import * as ns from "./dep.mjs";
    bump();
    console.log(x, ns.x);        // kiln: 2 1     node: 2 2

`refresh_self_imports` keeps a namespace in step only for a module importing *from itself*
(`self_namespaces`); a namespace of another module is snapshotted at link time and never caught
up. This is the `gtbndng-indirect-update` family in `language/expressions/dynamic-import/usage`,
72 rows.
