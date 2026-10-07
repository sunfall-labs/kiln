# Oxc `using` / SuppressedError follow-up evidence

This receipt covers source commit `6cdf4b4eaf0f133636a782b93896ea25c59a7209`.

## ManagedRuntime

The unchanged pinned Effect `ManagedRuntime` file completed under both engines: **9/9 cases passed, 0 failed, 0 skipped**, with exact Node/Vitest 5.0.3 case mapping and no source-transform gaps. The raw envelopes, logs, and build log are retained alongside this file. The original 5-second case budgets were unchanged; the 2400-second setting was only the runner's outer build/infrastructure allowance.

- Pinned Effect source: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Frozen CLI SHA-256: `f543ab7c5e0c77cef446557f7ddad7611cff895860489014b213324805b18a56`.
- Native binary SHA-256: `c4bf8d82a265016bd302fb4911a1c489b69a59e2eb3c724db629de035df41d49`.
- Verified compressed native archive SHA-256: `b04aaf6186906949b358a91d1beb38fa6a894eedf0d2d8807d4684841936307a`.
- Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
- Oracle: Node 26.8.1 / Vitest 5.0.3.

## Focused `using` fixture

`native-fixture.log` records `native: 1 matched, 0 refused, 0 diverged` for the new `conformance/native/oxc-using-context.mts` fixture. The fixture covers null async-dispose fallback, awaited-disposal scheduling, SuppressedError call/new behavior and branding, descriptor attributes, and a Proxy `newTarget.prototype` getter observed once.

## Known primitive-prototype gap

The diagnostic uses `Reflect.construct(SuppressedError, ["x", "y"], N)` where `N.prototype` is the primitive `1`. Node prints `true false`: the result inherits from `SuppressedError.prototype`, not `Object.prototype`. Kiln currently prints `false true` because `Vm::instantiate_for` falls back to `Object.prototype` when the constructor prototype is primitive. The constructor receives an already-created `this`, so recovering the specified fallback inside the wrapper by rereading `newTarget.prototype` would cause an extra observable read for Proxy new targets. The exact diagnostic and both commands' outputs are retained here. This limitation remains open; the constructor allocator needs to preserve the selected built-in default prototype while it performs `GetPrototypeFromConstructor`.

## Reproduction commands

```sh
node .scratch/oxc-followup/primitive-prototype-diagnostic.mts
kiln --one-shot run .scratch/oxc-followup/primitive-prototype-diagnostic.mts
```

The fixture command from the source checkout was:

```sh
scripts/native.sh conformance/native/oxc-using-context.mts
```
