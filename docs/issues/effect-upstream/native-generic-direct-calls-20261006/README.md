# Bounded generic direct calls

The large-module `generic_only` path skipped `known_callees` entirely, so even small, frame-free
functions in these two pinned Effect modules always used dynamic call dispatch. The bounded plan
now performs that whole-program proof when there are at most 128 functions and at most 8,192 IR
instructions, including the module entry. It does not enable typed or fast entries, clone bodies,
or infer argument types. The emitter's existing `direct_callable` guard chooses the generated
function call only for non-suspending, non-frame-publishing targets; other calls retain the
generic boxed path. `set_available` removes known targets absent from the emitted unit.

Pinned source checkout: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.

| Module | Functions | Function IR | Entry IR | Total IR | Known sites before | After |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `packages/effect/src/Equal.ts` | 21 | 1,029 | 102 | 1,131 | 0 | 24 |
| `packages/effect/src/internal/hashMap.ts` | 104 | 6,976 | 968 | 7,944 | 0 | 6 |

Source digests: `Equal.ts` `0fea9e7418d82fa04f3c790c8537abed3fd17d6529d1d467612c8bed66748303`;
`hashMap.ts` `1a57c3acbbb04d14fec2ef6221dab4a2d0567dd66646476ddc293bb8f537cfc3`.

Validation used `KILN_JOBS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 MBX_GC_AUTO=false mise exec -- mbx test -p kiln-backend-c --lib -- --test-threads=1` on base `efd9e68a8920d764ccc7df1f244d7591d74794d6` plus this change. Result: 337 passed, 0 failed, 2 ignored. Fixtures exercise a direct generic call followed by a dynamic callback, a captured cell under GC stress, a reassigned binding, and throw propagation. The focused planner tests also check the full-module budget and liveness fallback.

`diagnostic.log.gz` is the pinned module analysis output; `full-backend.log.gz` is the complete test stream, preserved byte-for-byte before compression.
