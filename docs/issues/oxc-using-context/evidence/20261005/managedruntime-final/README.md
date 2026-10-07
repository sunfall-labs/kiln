# ManagedRuntime validation on the final constructor candidate

The unchanged pinned `ManagedRuntime` file completed under the interpreter and native-release
engine: **9/9 cases passed in each**, with exact Node/Vitest 5.0.3 outcome mapping, no skips,
timeouts, mapping gaps, or source-transform gaps. The original per-case timeout budget was
unchanged; `--timeout 2400` is only the outer build/infrastructure allowance.

Candidate source commit: `c78e112a10622cdf67808daaff6ca677b6195964`.
Release CLI SHA-256: `230c616785037db74c12adb2e6d48b3d6182ba4c69860c7d84c8270b094d9550`.
Provider-manifest SHA-256:
`4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
Interpreter report SHA-256:
`caa582bc5a18e5cabaaa5cb4c46e74fa49f0c2e2828772da578775179fc5a520`.
Native report SHA-256:
`8647319cd6df971c4daec70bbef09f2e893dd9b1e46694f9baff6219289c170f`.

The runner used a pinned workspace and provider, one compiler/runtime worker, native-release,
verified zstd compression, and normal artifact-cache GC. Original native binary SHA, compressed
artifact SHA/size, CLI path, provider context and command envelopes are retained in `native.json`
and `native-report.json`. The binary/archive themselves are not committed.
