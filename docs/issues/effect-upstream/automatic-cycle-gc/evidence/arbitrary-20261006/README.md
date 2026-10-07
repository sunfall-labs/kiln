# Automatic-cycle-GC Arbitrary replay

This is the unchanged pinned `arbitrary/Arbitrary` interpreter file run against the frozen automatic-cycle-GC CLI. The runner completed the full registered inventory under its normal per-case deadlines and memory limit.

- Source commit: `ea95686fe704f60676b45d4ce0dad6d1c59a56e5` (clean at build)
- Build: `mbx build --release -p kiln-cli --offline --jobs 1`; systemd unit `kiln-effect-autogc-cli-1791269747`, exit 0, 160.633 seconds
- Frozen CLI SHA-256: `3ff14d18c5e4f9f6fe8097f7289641629f55dd8f77fc60d6a2dfb5c85afe1021`
- Pinned upstream corpus: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Upstream `arbitrary/Arbitrary` source digest: `b5e4399d15fd54e4187279fc7989ebc8c6304b9f2abefeaf9d7274e061b6bfa0`
- Harness SHA-256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`
- Runner commit/SHA: `ea95686fe704f60676b45d4ce0dad6d1c59a56e5` / `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`
- Invocation: `scripts/effect-upstream.py --engine interpreter --path arbitrary/Arbitrary --jobs 1 --runtime-jobs 1 --timeout 1800 --kiln-memory-mb 4096`; `--clock wall`; harness case and hook budgets were unchanged.
- Durable unit: `kiln-effect-arbitrary-autogc-20261006`, exit 0; runner wall time 108.180 seconds, child wall time 107.346 seconds.
- Result: 178 registered/recorded, 170 pass, 8 fail, 0 skip; no outer timeout, no incomplete cases, and no allocation failure. The eight failures are all original `TEST_TIMEOUT: 5000ms` outcomes.

The seven timeout IDs seen before the prior baseline allocation abort remain: Date constructive generation, Uint8Array declaration generation, bounded integer/BigInt edges, cardinality boundaries, built-in/canonical schema catalog, recursive sibling-order depth, and the 5,000-node mutually recursive component. The candidate also reached and timed out the final `Arbitrary > checkEffect > traverses a thousand lazy child shrinks without overflowing` case, which was not reached in the earlier run that aborted after 172 records. This is evidence that the previous allocation abort did not recur in this full-file run; it is not a claim that all cases pass or a measured peak-memory result.

The archived earlier comparison uses source commit `efd9e68a8920d764ccc7df1f244d7591d74794d6` and CLI SHA-256 `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`, with the same corpus/source digest, harness SHA, per-case deadlines, and 4096 MiB address-space limit. It recorded 172 cases (165 pass, 7 timeouts), then exited -6 after `memory allocation of 16777216 bytes failed` at 106.855 seconds. Its per-file JSON and compressed raw log are included here. The source/CLI differ, so this is a behavioral comparison, not a controlled memory benchmark.
