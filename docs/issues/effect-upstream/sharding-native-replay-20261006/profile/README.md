# Diagnostic profile (not acceptance)

This is a single `samply` 100 Hz profile of the already-built Sharding executable. It is diagnostic only: the sample was taken while the root gate was running, and the profiled replay recorded 59/66 rather than the unprofiled 61/66. The profile run therefore cannot be used to claim a runtime speed or a conformance result.

The profiled binary was the exact staged artifact from the acceptance run (SHA256 `082875197e0c4a50e857f56ae349dfdca5347e0263b36856e6e749e91ce22a36`). A symbol-retaining relink used the same frozen CLI, entry and provider. The build report showed generated-C output reused, app/prelude/runtime/launcher objects all hit, and a 2.855 s link. The symbolized and original executables had identical `.text` address `0x1713000`, size `0x0b002f03`, and SHA256 `7c36ffe225210e2421e5276e474f37df87d78d0feb26d49f14a4bf692022e828`. Every allocated ELF section matched byte-for-byte except `.note.gnu.build-id`; all PT_LOAD segments after the first were identical. The first PT_LOAD differs only because the build-id note differs. This makes symbol names applicable to the sampled code addresses without asserting that the relink is byte-identical as a whole file.

The sample contains 6,168 weighted samples on the native executable thread. Top self samples after resolving addresses against the identical-code symbolized image were spread across runtime work: `kiln_c99_gc_lookup_nursery` 225 (3.6%), `kiln_val_to_boundary` 182 (3.0%), `kiln_c99_get_own_kind` 152 (2.5%), `kiln_c99_invoke_method_dispatch` 146 (2.4%), `kiln_c99_property_read_own_with_hash` 136 (2.2%), `kiln_c99_new_object_sized` 133 (2.2%), and `kiln_c99_own_index_key_with_hash` / `kiln_c99_key_hash` 114 each (1.8%). No single leaf dominated.

Large inclusive counts are callers, not exclusive costs. The trace runs through `kiln_c99_call_inner` and `kiln_c99_invoke_closure_body` on most samples. Generated function IDs map to `source/packages/effect/src/internal/effect.ts` (`causePrettyErrors` and `causePrettyError`), `source/packages/effect/src/Scheduler.ts` (`afterScheduled`/microtask scheduling), and `source/packages/effect/src/cluster/Sharding.ts` (storage read loop). In particular, `Sharding.ts` function 77 covers the storage loop and appears in 2,296 inclusive samples (37.2%). That identifies likely workload regions for a later targeted profile; it does not attribute the timeout to one function or justify a source change by itself.

Artifacts:

- `profile.json.gz`: raw profile (SHA256 `3df61295a3456c11141100c81ab168f7e3f89139e7d0b1ea6b32a8a0787f56fc`).
- `profile.json.syms.json`: profile's symbol sidecar.
- `run.log`: complete profiled execution output and its 59/66 summary.
- `symbolized-relink.log`: linker/cache-hit receipt for the symbol-retaining image.
