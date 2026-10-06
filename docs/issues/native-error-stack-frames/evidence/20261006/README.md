# Native direct-call frame validation

This receipt records the reviewed direct-call frame fixes and their focused validation. It is evidence for these selected tests only, not a full native corpus or gate result.

## Source and frozen executable

- Worktree source commit: `b7bfc66bb2f77991cb09b62913215635bbd12478` (`fix/suppressed-disposal-prototype`).
- Source tree: `eecfc5236fa6070ff6139d08955f9ca171247553`.
- Direct-call frame implementation commits: `6d7999fb88203681ca09abc6d90e324420f86b18` and `9d6bc166b0113415e49e2b72b0b76499ca126a82`.
- `b7bfc66b` additionally cherry-picks root commit `3faacb4c`, a separate SuppressedError/Error prototype-identity guard needed for the final unchanged stack probes. The frame tests themselves cover direct-call frame publication and typed status handling.
- Frozen release CLI: `.scratch/disposal-validation/stackdiag/candidate/final-v4/kiln`, SHA-256 `08aacd56723d751abb3bc32a160ba7e606dd3b323d4f51b7010047e99d6beb24`.
- Provider manifest: `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/formatter/toolchain/provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/kiln-native-provider.json`, manifest digest `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`, provider archive SHA-256 `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.
- Rust 1.97.1 and mbx were selected explicitly. The build/test environment used `RUSTUP_TOOLCHAIN=1.97.1`, `MBX_GC_AUTO=false`, `MBX_SCHEDULER_CPUS=3`, `CARGO_BUILD_JOBS=1`, `KILN_JOBS=1`, and `RAYON_NUM_THREADS=1` with the shared integration MBX cache and target.

## Results

- The four direct typed-call regressions passed: typed result remains live after return; typed failure preserves callee attribution; recursive typed self-call retains its closure for the frame; and a successful call after a caught typed failure does not reuse stale failure status. Full focused log: `native-direct-typed.log`, SHA-256 `fabbdcfaf344738046e12e202663525f51370cdda348f0eacaec4384c01b1a19`.
- The four existing non-callable diagnostic regressions passed: caught error state does not leak; cross-module error keeps the innermost site; call-site and callee context are retained; nested error names the innermost function. Full focused log: `native-non-callable.log`, SHA-256 `6a6730e04fa67ce124099fa53da706be197f164434c9faaff7fc87ed956a3335`.
- On the frozen CLI, all four unchanged native stack probes matched: `error-automatic-stack.mjs`, `error-capture-stack-trace.mjs`, `error-stack-source-coordinates.js`, and `commonjs-stack-activation.cjs`. Summary log: `native-stack-four-summary.log`, SHA-256 `cbedb806e712237d31ad68f60ec8b198878e67e7cbec5ed37ad6291e118a2707`.
- The final release build completed successfully; compressed complete build log is `final-release-build.log.gz` (uncompressed log SHA-256 `5b1c857ce5879745b1bc4fa8bf5a1493ff6667746b74f0b23781dee307f48562`).
- An earlier run before the independent prototype-identity guard was 3/4, with only `error-automatic-stack` diverging on the SuppressedError prototype check. That superseded result is retained as `earlier-stack-three-of-four.log.gz`; it is not the final verdict.

## Reproduction commands

From this worktree, using the pinned environment above:

```sh
mbx test -p kiln-native native_direct_typed -- --nocapture --test-threads=1
mbx test -p kiln-native native_non_callable -- --nocapture --test-threads=1
scripts/native.sh --kiln .scratch/disposal-validation/stackdiag/candidate/final-v4/kiln \
  conformance/native/error-automatic-stack.mjs \
  conformance/native/error-capture-stack-trace.mjs \
  conformance/native/error-stack-source-coordinates.js \
  conformance/native/commonjs-stack-activation.cjs
```

The native command used the explicit frozen provider manifest above. The focused test logs preserve the actual selected test names and terminal summaries.
