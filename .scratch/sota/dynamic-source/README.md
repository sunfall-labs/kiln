# Dynamic source boundary evidence

Measured from commit `128adc83` in the owned `kiln-source-names` worktree. The temporary harness uses local path dependencies on `frontend-oxc`, `kiln-hir`, `kiln-ir`, `kiln-interpreter`, and `kiln-runtime`; rerun from this directory with:

```sh
mise exec -- mbx run --manifest-path .scratch/sota/dynamic-source/Cargo.toml --offline
```

The harness callback prints UTF-16 units received by `EvalSource.source`. It exercises three dynamic `eval` inputs built from `String.fromCharCode(0xd800)`: a raw surrogate inside a quoted literal, an escaped `\\uD800` spelling, and a raw surrogate outside a literal. The current boundary is:

- raw quoted source arrives as `34,65533,34` (lossy U+FFFD);
- escaped source arrives as ASCII `34,92,117,68,56,48,48,34` and remains representable;
- raw outside-literal source arrives as `65533` before the compiler can issue the language SyntaxError.

The exact output is in `dynamic-source-output.txt`.

The same frontend→IR→interpreter harness executes `auto-accessor-source.ts`; current output is in `auto-accessor-output.txt`. It exposes a separate runtime/property issue: the lone-surrogate accessor first reads `undefined`, while U+FFFD reads `22`; after writes, reads are `101 202`.

`node-probe.js` is the Node oracle for raw and escaped quoted strings, tagged-template cooked/raw contents, regexp source, comments, invalid outside-literal input, Function source reflection, and low surrogates. Node reports raw and escaped quoted values as UTF-16 unit `55296`, raw tagged-template cooked/raw both `55296`, raw regexp source `55296`, comments unaffected, and raw outside-literal input as `SyntaxError`; `ShadowRealm` is unavailable in the installed Node runtime.
