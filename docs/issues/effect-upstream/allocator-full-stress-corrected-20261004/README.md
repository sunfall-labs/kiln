# Corrected full native GC-stress run (2026-10-04)

The complete top-level native probe inventory was run with `KILN_NATIVE_GC_STRESS=1`, using the frozen release CLI and corrected full-GUI provider manifest. The original probe runner was used without timeout or skip overrides. Final result: **600 matched, 5 refused, 0 diverged, 45 intentional stress-skips**; all 650 inventory entries are accounted for. Refusals remain the native backend backlog, not mismatches.

`native-full-gc-stress.log` is the complete runner output. `inventory.txt` is the exact 650-name input inventory. `receipt.json` records source, runner, CLI, and provider digests. `provider-manifest.json` records the packaged provider identity used by the native runs; the archive itself was external and is identified by its SHA-256 in the receipt.

The run used the frozen CLI at `~/Work/kiln-effect-allocator-milestone-gate/.scratch/http-fixed-candidate-toolchain/kiln-16958abcd29f328d23bb789fc188f0e55a065e349b0b77736ce0354ba1820ceb` and provider manifest SHA-256 `5eaa7d77a4e17c0d376c48b773a08870de12a0768fb3c3d69f3129fdb6e5fd2e`. The manifest points to provider archive SHA-256 `84eead2f3d7049671c99c6bfdfe6aeea2e50adc371de40f099e0e8dca4f16959` built from source SHA-256 `08b2ea2726c6500f5c11e32bb7093be7f8983a0edcd0d1d71120009bb08ea820`.
