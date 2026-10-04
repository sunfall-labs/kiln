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
