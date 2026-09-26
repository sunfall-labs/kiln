# 01. Mark kiln extensions and add the switch that removes them

Status: resolved

ADR 0068, rules 1, 2, 4, 5 and 6.

1. Add a "kiln extension" mark to the built-in member description (`kiln-builtin-surface`), and
   mark `parallelMap` on `Array.prototype` and `%TypedArray%.prototype`.
2. Both engines skip installing marked members when `KILN_EXTENSIONS=0`; `kiln build
   --no-kiln-extensions` records the same in a binary. With either, the built-ins are exactly
   Node's.
3. The surface inventory reports marked members in their own column, not as "extra (node lacks
   it)".
4. Check every marked member is non-enumerable, as a test over the description.
5. `packages/kiln-concurrency/index.js` already defines its fallback only when the method is
   absent; keep that, and add a test that under kiln the engine's method is the one in place.

## Acceptance

- `KILN_EXTENSIONS=0 kiln run` and a `--no-kiln-extensions` binary: `'parallelMap' in []` is
  `false` and `Object.getOwnPropertyNames(Array.prototype)` matches Node's, in both engines.
- Test262, `scripts/thirdparty.sh` and (once it exists) the deploy corpus pass identically with
  extensions on and off.

## Comments

2026-09-26, branch `alee/kiln-extensions`.

**The mark.** `kiln_builtin_surface::KILN_EXTENSIONS` holds one row per extension (owner, typed key,
attributes, deciding ADR): `Array.prototype.parallelMap` and `%TypedArray%.prototype.parallelMap`.
It is keyed by owner name because `%TypedArray%.prototype` has no `PrototypeId` (the TypeScript
bootstrap installs it). A `const` check holds every row non-enumerable, unique, and -- for a
`PrototypeId` owner -- equal in attributes to its `PROTOTYPE_SURFACE` row;
`every_kiln_extension_is_non_enumerable` checks the same thing as a test.
`PrototypeMember::is_kiln_extension()` derives the mark for a surface row.

**The switch, one check per engine.**
- Interpreter: `kiln_extensions_installed()` (in the surface crate) reads `KILN_EXTENSIONS` once per
  process. Every interpreter lookup goes through `installed_prototype_member(s)`, which leaves out
  marked rows when the switch is off. `typed_array_generic` asks `is_withheld_extension`.
- Native: the `builtin_surface!` row for `parallelMap` carries `kiln_extension`, and a `const`
  assertion holds that mark equal to the description. The rows emit
  `kiln_c99_builtin_member_is_extension[]`. `surface.c` has `kiln_c99_kiln_extensions_on()`, which
  reads the env once, or is a constant 0 under `KILN_REALM_NO_EXTENSIONS`. Withheld rows never enter
  the member index, and own keys skip them. `kiln_c99_array_method`, `kiln_c99_is_array_member` and
  the built-in own-property answer ask `kiln_c99_extension_withheld`. Both `parallelMap` lowerings
  in `call_method.rs` are guarded.
- TypeScript bootstrap: both engines publish `__kilnExtensions`. The new first part of the common
  bootstrap, `extensions.ts`, captures and deletes it. `typedarray.ts` installs through
  `defineExtension`. A `kiln-builtins` test holds that file to the description in both directions.
- `kiln build --no-kiln-extensions` (build only; refused with `--shared`) is carried through
  `Request::NativeBuild` (serde default, skipped when false) to
  `NativeRealmPolicy::without_kiln_extensions` and then to `EmissionPolicy::no_kiln_extensions`.
  That policy defines `KILN_REALM_NO_EXTENSIONS` in the prelude and mixes into the module cache
  hash only when it is set.
- The CLI serves a command in-process when `KILN_EXTENSIONS=0`. A resident daemon has its own
  environment, so the switch would otherwise not apply.

**Inventory.** `scripts/native-surface-inventory.py` enumerates the interpreter a second time under
`KILN_EXTENSIONS=0`. The members that disappear are recorded as `kiln_extensions` and reported in
their own "kiln extension (ADR 0068; intentional)" row, never as "extra". Checked against the
interpreter: it finds `Array.prototype.parallelMap` and `BigInt64Array.prototype^.parallelMap`
(the walk's key for `%TypedArray%.prototype`).

**Probes.** `scripts/native.sh` now reads a first-line `// native-env: NAME=value` and runs both
engines under it (a minimal extension).
- `conformance/native/kiln-extensions-off.ts` runs under `KILN_EXTENSIONS=0`. It checks
  `'parallelMap' in []`, and it checks that `getOwnPropertyNames(Array.prototype)` equals Node
  v26's list (40 names). It also covers descriptors, `hasOwnProperty`/`Object.hasOwn`,
  `Reflect.ownKeys`, the typed-array prototype, the TypeError from both lowerings, and that the
  bridge global is gone.
- `conformance/native/kiln-concurrency-engine-method.ts` imports `@kiln/concurrency` and shows the
  engine's method stays in place: it is named `parallelMap`, where the package fallback would be
  named `value`.
- A `kiln-cli` one-shot test builds with `--no-kiln-extensions` and runs the binary with
  `KILN_EXTENSIONS=1`. The binary still has Node's surface.

**Verified.**
- `mbx test --offline` passed for `kiln-builtin-surface` (16), `kiln-builtins` (95),
  `kiln-proto` (15), `kiln-interpreter` (190), `kiln-backend-c` (175 + 36; the prelude ABI
  snapshot was updated for the two new functions and the flag table), `kiln-daemon` (108 + 1),
  `kiln-native` (217 + 4) and `kiln-cli` (155 + 1 + 2 + 8).
- `scripts/native.sh` (debug binary): 224 matched, 1 refused (`shadow-realm`, which was already
  refused), 0 diverged.

**Not verified here.** Test262, `scripts/thirdparty.sh` and the deploy corpus with extensions on
and off are left to the integrator. The gate was not run.
