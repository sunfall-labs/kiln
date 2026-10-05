# SuppressedError and Proxy-super follow-up

This candidate is based on `e31d3a221e47b335fcbb55f7c7976ecadcf1c987` in the isolated
`suppressed-newtarget` worktree. The implementation/fixture diff before this note was added had
SHA-256 `b6a7164d3163d918492efa77fb890931f038741e44354a76f067de85da79a37d`.

The public `SuppressedError` is a Proxy around an ordinary three-argument function. It constructs
a branded Error through the shared Error constructor, then defines the optional `message` before
`error` and `suppressed`. The Proxy construct path reads `newTarget.prototype` once, uses an
object/function result, and falls back to the public `SuppressedError.prototype` for a primitive.
The apply path returns a fresh branded Error. This avoids the native kernel's current
SuppressedError tag behavior, which interprets the first argument as an Error message instead of
the `error` payload.

Class `super()` with Proxy heritage has two relevant execution paths. A construct trap's object
result is already the `[[Construct]]` result, so both engines preserve its prototype. A transparent
Proxy around a compiled constructor schedules a VM frame; the interpreter marks that frame as a
super-call continuation and lets normal frame completion adopt the constructor result, without
re-entering the VM run loop. Native generated code forwards Proxy super calls through
`[[Construct]]` with the active derived `new.target`. Prototype adoption is limited to native
builtin superclasses that allocate an object separately; returned Proxy/function objects retain
their own prototype.

Validation on the final focused candidate:

- `node conformance/native/oxc-using-context.mts` passed.
- Release CLI build: `mbx build --jobs=1 --release -p kiln-cli`, with
  `MBX_GC_AUTO=false`, `CARGO_BUILD_JOBS=1`, `KILN_JOBS=1`, and `RAYON_NUM_THREADS=1`.
- CLI SHA-256: `230c616785037db74c12adb2e6d48b3d6182ba4c69860c7d84c8270b094d9550`.
- Ordinary native parity reported `1 matched, 0 refused, 0 diverged`.
- Native GC-stress parity reported `1 matched, 0 refused, 0 diverged`; no stress probes were
  skipped.
- Raw native output: `native.log` (SHA-256
  `e28ff78be08fb06535f3693219a8827b0442036407945a5af20c273c487871ff`).
- Raw GC-stress output: `native-stress.log` (SHA-256
  `49736fd76ac0966dc9d304728ca2b63c9d8cbe4c4cfdc817713c33fd9a4d7e84`).

The fixture checks call/new, own payload and message descriptors, Error branding/inheritance,
message conversion ordering and single evaluation, omitted/undefined/null messages, extra options,
primitive and explicit `Object.prototype` new-target prototypes, one observable Proxy prototype
read, and both trap and transparent Proxy `super()` behavior (including explicit parent-object
returns and derived `new.target`).

The final-source ManagedRuntime 9-case interpreter replay passed 9/9 with all cases mapped to
Node/Vitest 5.0.3. The matching native-release replay is in progress; its receipt will be added
separately when it completes. No upstream budgets were changed.
