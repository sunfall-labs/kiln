# Corrected HTTP provider focused stress validation

This run checked the 20 HTTP probes that diverged in the full 2026-10-04 GC-stress run, plus four representative GUI probes, using the corrected full-GUI provider package. The existing frozen CLI was reused; no Rust build or archive replacement took place in this worktree.

Result: **23 matched, 0 refused, 0 diverged; 1 intentional stress skip**. All 20 HTTP probes matched under `KILN_GC_STRESS=1`. Three GUI probes matched under stress: `gui-accessibility.ts`, `gui-reactivity-gc.ts`, and `gui-widgets.ts`. `gui-reactivity.ts` declares `// kiln-native: stress=skip`, so the official runner skipped its native stress invocation. This was not overridden. The separate ordinary GUI coverage belongs to the existing ordinary-corpus evidence, not this stress result.

The failed full corpus receipt remains unchanged in the parent directory; this focused run only verifies the provider correction for its HTTP failure family. It does not turn the earlier full run into a pass.

## Provenance

- Source checkout for the frozen CLI: `3d41ebb21f316eb4c8b691eb08904eb2bdf4a3d1`.
- Frozen CLI SHA-256: `f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0`.
- Corrected provider manifest SHA-256: `5eaa7d77a4e17c0d376c48b773a08870de12a0768fb3c3d69f3129fdb6e5fd2e`.
- Corrected provider archive SHA-256: `84eead2f3d7049671c99c6bfdfe6aeea2e50adc371de40f099e0e8dca4f16959`.
- Corrected provider source SHA-256: `08b2ea2726c6500f5c11e32bb7093be7f8983a0edcd0d1d71120009bb08ea820`.
- Runner SHA-256: `0fef47bb437f658cc03f4f8133a1b25496cbb269defcd9a0904c68990ab6c5d0`.
- The exact summary log is `run.log` (SHA-256 `6dd8178d147442f56872d201e08a9792eeae9e0d0e17a0ea5850d39ee7aa5b8d`).

The invocation used `KILN_NATIVE_GC_STRESS=1`, `KILN_JOBS=2`, and `RAYON_NUM_THREADS=1` with `scripts/native.sh` and the 24 explicit paths below:

