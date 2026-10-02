# Matched native startup: CLI74 vs CLI79

This is a paired startup diagnostic, not a benchmark-baseline update. It records the exact inputs, output checks, launch samples and binary/source provenance so the comparison can be repeated without relying on temporary logs.

## Provenance

- Host: CachyOS Linux 7.2.6-1, x86_64, AMD Ryzen 7 5800H with Radeon Graphics.
- Toolchain: rustc 1.97.1 (8bab26f4f 2026-07-14), Clang 22.1.8.
- Both artifacts were built with `--profile native-release`; the timed launches include process startup.
- CLI74 source commit `7780d181418af3cfa52060525cf90fa62f46b7f4`; CLI SHA-256 `b55f79b30eccae1da888c54e7c551896d17608f85c58d9ad2bdf355114e744a3`.
- CLI79 source commit `613d5f87`; source snapshot SHA-256 `c13b3291ae0178fc5394d6fab2d13badc266def137344ece79bdeefef6b5a20a`; CLI SHA-256 `f10f96748fca56c94e4c6f9919a5c832797996fbdc4d84c98e8c83246812c9f0`.
- Workloads are checked in under [`workloads`](workloads/). `startup-tiny.mjs` SHA-256 `6ce37a207702f25bc304bdfc8142c23acab4bda1794a95bd5d328a31287ebb15`; `ms.bundle.js` SHA-256 `8f38f4b197a1728e97c058156ebe4272a7c26628862488b25861957aee0781d0`.
- The ignored `target/startup-evidence/` directory retains the four binaries. Their hashes are in the raw JSON file.

## Build and timing commands

The frozen CLI artifacts built each workload with this command shape, with no Rust rebuild:

```sh
<cli74-or-cli79> --one-shot build <workload> --profile native-release --output <binary>
```

The environment pinned `RUSTUP_TOOLCHAIN=1.97.1`, `CARGO=~/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/cargo`, and `KILN_JOBS=1`. The timing driver ran three warmup launches per binary, then 40 paired rounds per workload, alternating which binary launched first. Each sample used `perf_counter_ns` around a fresh `subprocess.run` with stdout/stderr captured. Nonzero exit or differing output failed collection. Full launch samples, per-sample output hashes, and the protocol are in [`thirdparty-startup-timings.raw.json`](thirdparty-startup-timings.raw.json).

## Results

Medians/minima/p90s are milliseconds. P90 uses nearest-rank indexing over 40 samples.

| Workload | CLI74 median | CLI79 median | 79 / 74 | CLI74 min / p90 | CLI79 min / p90 |
|---|---:|---:|---:|---:|---:|
| `tiny` | 3.870 | 3.944 | 1.019 | 3.653 / 4.233 | 3.659 / 4.262 |
| `ms.bundle.js` | 5.721 | 5.719 | 1.000 | 5.224 / 5.936 | 5.319 / 5.998 |

Both arms produced identical stdout and exit 0 for each workload. These paired samples show no consistent added startup cost from CLI74 to CLI79. The `ms` workload's Linux startup is about 5.72 ms for both artifacts; the earlier 2.00 ms Sep 13 baseline was recorded without platform/source provenance, while the documented Sept 14 baseline was on a Mac M5 Pro. This result supports a platform-qualified baseline rather than attributing the difference to a recent runtime change. Because the host may have had concurrent parity work, use these samples as matched startup evidence, not an isolated performance claim.
