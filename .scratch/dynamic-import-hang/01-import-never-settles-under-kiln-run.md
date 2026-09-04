# `import()` does not resolve under `kiln run`

**Status:** open · **Severity:** medium — reported, but the promise never settles

**CORRECTION (same session).** This was first written as "silent, no diagnostic, exit 0". That
was wrong, and the error was in the probe rather than in kiln. The probes piped output through
`grep -v "^kiln:"`, which strips exactly the diagnostic line, and the `exit=0` beside them was
`grep`'s status, not kiln's. Measured properly:

    $ kiln run main.mjs; echo $?
    A
    B
    kiln: ... unsupported dynamic import: no directory was recorded to resolve `import()` against
    1

So it fails loudly, names the cause, and exits non-zero. What remains true is that the promise
never settles, so a `.then`/`.catch` handler on it never runs -- the module stops at the failure
instead of reporting through the promise the way the specification does.

## Repro

    // main.mjs
    console.log("A");
    import("./dep.mjs").then(m => console.log("then", m.x), e => console.log("rej", e.message));
    console.log("B");

    $ kiln run main.mjs        # node prints A, B, then `then 1`
    A
    B
    kiln: ... unsupported dynamic import: no directory was recorded to resolve `import()` against

Neither handler runs, and the module stops there. The same holds for `await import(...)`.

## The cause

`ModuleLoader` is `fn(&str) -> Result<ModulePlan, ModuleLoadError>` and carries no referrer. The
only implementation is `kiln-conformance`'s, which resolves against a thread-local base -- exact
for Test262, where every fixture is a sibling of the test that imports it, and unset under `kiln
run`, which is the message above.

The type's own comment says the referrer-aware service object that replaces it "is part of the
remaining module-realm kernel migration", and that a filesystem-backed loader "must not emulate
one with ambient state". So the fix is that migration, not a thread-local in the CLI: faking a
referrer would resolve a relative import from a nested module against the wrong directory, which
trades a visible failure for a silent wrong answer.

## Two smaller things worth doing meanwhile

* **Report through the promise.** `import()` hands back a promise and the specification reports
  every failure through it. Both the no-loader and unsupported paths raise a machine error
  instead, so `.catch` never runs. Rejecting would let a program handle it, and an unhandled one
  is reported anyway.
* **`ExecutionError::NoModuleLoader` looks unreachable** from the CLI, because the conformance
  source compiler installs a loader for it. If a host really has none, that path has never been
  exercised.

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
