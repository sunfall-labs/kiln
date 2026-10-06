# Hash interpreter timeout profile (diagnostic)

This records a module-qualified CPU profile of the unchanged Hash law timeout. It is diagnostic evidence only: the unsampled run is the outcome reference, while attaching `samply` materially perturbed runtime and caused additional original-deadline timeouts.

## Frozen inputs and unsampled result

- Effect corpus pin: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Hash source SHA-256: `9fbc376ce19f8de3190eb9de5e8fa5b8bd2698cf16d3617178bf401eb5f3ea0c`.
- Kiln CLI SHA-256: `d4046b9e5d4bceb6903aca62e154b5782a7c7831a9bacb90b664e0495c04d655` (built from compiler source `ecfc065b`; runner checkout `827fecf58a1a31d9bc5cd8f27f9c95604cf5509e`).
- Oracle: Node `26.8.1`, Vitest `5.0.3`; oracle/mapping digests are retained in `unsampled/report.json`.
- Command: `python3 scripts/effect-upstream.py --workspace <frozen-workspace> --out <out> --engine interpreter --path Hash --kiln <frozen-cli> --jobs 1 --runtime-jobs 1 --timeout 1800 --kiln-memory-mb 4096`.
- Every one of the original 25 case IDs mapped; 24 passed and one failed at its unchanged 5,000 ms per-case deadline. The failing case was `Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`. The interpreter process exited 1 after 60.64 s; supervisor wall time was 61.53 s. Full report, case records and raw output are in `unsampled/`.

## Sampled run and attribution

`samply` attached to the already-running interpreter in a separate bounded service. Target and sampler unit receipts, marker stream, raw log, profile and symbol sidecar are included in `sampled/`. The profile window spans the failing case from its prior marker at 41.845 s to its timeout marker at 75.412 s: 33.567 s and 2,052 worker samples. The sampled run reported 21/25, with four original 5 s timeouts; therefore none of its case counts or elapsed times are a performance result. The unsampled 24/25 remains the correctness baseline.

Top sampled leaf symbols in that interval were `Vm<CaptureHost>::run_bounded` (473/2,052, 23%), `Value::clone` (73), `StringUnits` equality variants (131 combined), `Value` drop glue (67), `symbol::is_internal_key` (66), `ObjectData::lookup_key` (61), `IntrinsicKey` equality (59), libc `_int_malloc` (59), libc `unlink_chunk` (54), and `WeakCensus<ObjectData>::register` (51). The sample stacks frequently pass through promise construction, generator resumption, async driving, and microtask draining; those inclusive ancestors do not establish that the scheduler itself is the bottleneck.

The generic-looking `fun_*` symbols were resolved through the profile resource table to **libc.so.6**, not generated module functions: `fun_a6e20` is `_int_malloc` (59 leaves), `fun_a4f10` is `unlink_chunk` (54), `fun_a4fe0` is `_int_free_create_chunk` (27), `fun_176400` is `__memcpy_avx_unaligned_erms` (33), and `fun_175ca0` is `__memcmp_avx2_movbe` (22). This module qualification is essential; these labels are not kiln's generated `fun_*` IDs.

For the hottest interpreter leaf, `llvm-addr2line -f -C -i` mapped sampled instruction addresses in `kiln_interpreter::vm::Vm<CaptureHost>::run_bounded` to `crates/interp/kiln-interpreter/src/vm/execution.rs`: `0x241e9b0` and `0x241e9be` at line 162 (opcode dispatch), `0x241e967` at line 152 (opcode fetch), and `0x2429157` / `0x2429080` at lines 305 / 304 (binary-operation operand handling). A smaller property-read leaf (`0x242c554`) maps to the `GetProperty` route at line 1841 and inlined `prepare_property_read_key` in `vm/object.rs:1061`. `0x242748b` is an inlined caller ancestor through `construct_as`/`construct.rs`, appearing in most stacks; its frame count is not a leaf instruction count.

These observations point to interpreter dispatch, property/key equality, allocation, and weak-census work occurring in the failing computation. They do not by themselves establish a causal optimization target. Do not infer test semantics or final compiler behavior from this profile; rerun against the final combined CLI before proposing a change.

The first attempt placed target and sampler in one 4 GiB service and was OOM-killed. Its OOM journal and failure receipt are retained for completeness; the successful attach-based sample used separate bounded target and sampler services.

## Files and integrity

- `unsampled/`: exact 25-case report, per-case JSON/log, runner job and supervisor receipts.
- `sampled/`: sampled target job/command scripts, status receipts, marker stream, profile and symbol map, profile-analysis JSON, and failed first-attempt OOM evidence.
- `SHA256SUMS`: integrity hashes for every retained file except the manifest itself.
- No CLI executable or workspace cache is included; the immutable CLI digest above identifies it without committing a 303 MB binary.
