# Reset and top-level-await validation

The unchanged pinned Effect reset-sensitive selections passed on the interpreter after the module-generation/TLA changes. The runner reported 195/195 pass, zero skips, zero timeouts, and zero Node/Vitest inventory, mode, or outcome mismatches.

| Selection | Cases |
| --- | ---: |
| `Config` | 173/173 |
| `EffectKeepAlive` | 2/2 |
| `StackTraceLimit` | 4/4 |
| `rpc/RpcClient` | 11/11 |
| `schema/HMR` | 5/5 |

The exact command was:

```sh
env MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 \
  python3 scripts/effect-upstream.py \
  --workspace ~/.cache/kiln/effect-generation-reset-validation \
  --out .scratch/effect-generation-reset-final \
  --engine interpreter \
  --path Config --path EffectKeepAlive --path StackTraceLimit \
  --path rpc/RpcClient --path schema/HMR \
  --kiln ~/Work/kiln-generation-reclaim/target/release/kiln \
  --jobs 1 --runtime-jobs 1 --timeout 1200 --kiln-memory-mb 8192
```

Provenance:

- CLI SHA-256: `da203a6bc8c3f64db49cbcb903c6151af78ef6061b6fd07d5d14d8983ab0d897`
- Source commit: `c02b2d40221cb2e8d9bc0ef3e27e3aa744cc607d`; the CLI was built from the identical source contents immediately before that commit.
- Pinned Effect source: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Runner SHA-256: `02d82de635d748e14309b8072524c1e78d71d8676bfbd06f8d1dc9c03c2d9287`
- Node/Vitest oracle SHA-256: `f811d785382d5c113482816aca4925d701c230e2f4ba5e3f690b5e911e1fb9a6`

`report.json` is the complete runner report. The five `*.log` files preserve each raw interpreter stream. Focused module-evaluation and interpreter crate results are recorded in the implementation review; this report covers only the five upstream selections above.

## Focused validation

The same source commit also passed the complete module-map integration target (`29 passed`) and interpreter crate tests (`255 passed, 1 ignored`). The reset-during-TLA regression passed independently. The A/B/C wake-order regression has a durable red/green pair: with the previous first-pending wait, the watchdog recorded `a:start,b:start,b:end,watchdog`; with wake-on-any-dependency settlement, it recorded the expected `a:start,b:start,b:end,c,a:end,root`. The engine-private TLA test suspends twice, checks an ordinary module sees `__kilnPrivate` as absent during both pauses, checks the resumed engine module sees it, and checks rejection removes the global exposure. A helper test also checks exact restoration of a pre-existing global property.

The focused command forms were:

```sh
env MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- \
  mbx test -p kiln-module-eval --test module_map --jobs=1 -- --nocapture
env MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- \
  mbx test -p kiln-interpreter --jobs=1 -- --test-threads=1
```

`module-map-29.log` and `interpreter-255.log` contain full test output; `focused-reset-tla.log`, `any-dependency-red.log`, `any-dependency-green.log`, `private-window-helper.log`, and `private-window-tla.log` retain individual regressions. The red run temporarily restored only the old first-pending wait behavior, then the tested shared-wake implementation was restored and rerun green before commit.
