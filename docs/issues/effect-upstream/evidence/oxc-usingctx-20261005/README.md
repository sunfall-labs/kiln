# Oxc `usingCtx` and ManagedRuntime evidence

The compiler-owned TypeScript helper and shared `SuppressedError` facade were validated against
the pinned Effect `ManagedRuntime` file. Both engines ran the unchanged nine-case inventory and
matched all nine Node/Vitest mappings and outcomes. No cases were skipped or timed out.

## Provenance

- Source commit: `44da6111` (`provide compiler-owned Oxc usingCtx helper`).
- CLI: `target/release/kiln`, SHA-256
  `da82073a771c2a0fb208da85a2dcf7e6aab997a6fbb8d4d557232cedbc40c07f`.
- Runner: `scripts/effect-upstream.py`, SHA-256
  `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`.
- Pinned source workspace dependency identity: SHA-256 of `.dependencies` is
  `76e9198fb4711b1a00e23faf68e37f35a7df4a1252b2fad2496159c5db27d596`.
- Native provider manifest: SHA-256
  `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
- Native profile: `native-release`; `--jobs 1 --runtime-jobs 1`; 8192 MiB process address
  limit; upstream case budgets were unchanged. The runner's 2400-second outer timeout is only an
  infrastructure allowance.
- The native executable was zstd-compressed and round-trip verified before the raw binary was
  removed. Its SHA-256 was
  `4eb8534ef9a36fbcf3d61b8595bb35f8eca14403efa6d3d2d016385e119d98da`; the verified archive SHA
  is `dd93902165e7fae86c4d35e32c873c52a9c48dcae7f78f97367e775658eb165b`.

## Commands and results

Interpreter:

```sh
scripts/effect-upstream.py \
  --workspace .scratch/managedruntime-source \
  --out .scratch/managedruntime-interpreter \
  --engine interpreter --path ManagedRuntime \
  --kiln "$PWD/target/release/kiln" \
  --jobs 1 --runtime-jobs 1 --timeout 600 --kiln-memory-mb 8192
```

Result: 9 passed, 0 failed, 0 skipped; all nine official Node/Vitest cases mapped, with no mode,
outcome, inventory, or source-transform-gap mismatches.

Native:

```sh
scripts/effect-upstream.py \
  --workspace .scratch/managedruntime-source \
  --out .scratch/managedruntime-native \
  --engine native --path ManagedRuntime \
  --kiln "$PWD/target/release/kiln" \
  --provider-manifest ~/Work/kiln-effect-final-integration/.scratch/native-pool-frozen-result-long/toolchain/provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/kiln-native-provider.json \
  --native-profile native-release --compress-native-artifacts --cache-gc \
  --jobs 1 --runtime-jobs 1 --timeout 2400 --kiln-memory-mb 8192
```

Result: 9 passed, 0 failed, 0 skipped; all nine official Node/Vitest cases mapped, with no mode,
outcome, inventory, or source-transform-gap mismatches.

The committed `conformance/native/oxc-using-context.mts` additionally passed in the interpreter
and native-release executable. It covers reverse disposal order, awaited async disposal, sync
fallback for `await using`, SuppressedError payload descriptors, message coercion and omitted
message behavior, Symbol rejection, constructor arity/static inheritance, call form, and subclass
prototype identity. The direct TS smoke also checked nested suppression order. Its native executable
SHA-256 was `89da9313ed426889144772ed701afdfa2b9602a5168d5a1fdc82e9c3b76612ce`; the conformance
fixture executable SHA-256 was
`f35c8996c6b6295ff63508d9ddad080867fd937854a3f5c8d8ffc6ee19302599`.

The JSON envelopes and complete stdout logs for both ManagedRuntime runs are retained beside this
file. The separate native build log records a successful build before execution.
