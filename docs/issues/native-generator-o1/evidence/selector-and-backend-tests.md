# Bounded generic O1 tier: selector evidence

This candidate adds a separately bounded O1 bucket for loop-heavy generic bodies that publish or retain VM frames. It preserves the existing frame-free selection bucket and boxed body ABI. It does not add typed/direct entries or alter call/frame semantics.

## Pinned Sharding diagnostic

The ignored, one-off selector diagnostic was run against the pinned Effect source at:

`~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace/source`

Command:

```sh
KILN_EFFECT_SOURCE_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace/source \
MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 \
mise exec -- mbx test --jobs=1 -p kiln-backend-c --lib \
  diagnose_pinned_sharding_generic_optimization_selection -- --ignored --nocapture --test-threads=1
```

Result: **1 passed**. Sharding had 117 functions and 12,662 bytecode instructions. Proto 77 (anonymous `sendOutgoing`-adjacent loop profile target) has 916 instructions, `generator=true`, `publishes_frame=true`, and was selected by the bounded loop tier. No library-specific name allowlist is used.

## Backend library suite

The complete raw output is in `backend-lib-dc61153a.log` (SHA-256 `223a17d6a81bc0d8252a4b40eb9b81cbcbce66b5ad49b11042bbece7857bdf76`). Command:

```sh
MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 \
mise exec -- mbx test --jobs=1 -p kiln-backend-c --lib -- --test-threads=1
```

Result: **334 passed, 0 failed, 2 ignored**, in 305.55 seconds. The new executable emission fixture constructs and resumes a selected generic looping generator with `KILN_GC_STRESS=1`; it checks the yielded object and that the generator remains suspended. The fixture also asserts the selected body remains boxed and has no typed clone.

Formatting and whitespace validation passed with `cargo fmt --all -- --check` and `git diff --check`. Toolchain: Rust 1.97.1. This run is correctness evidence only; no generated CLI or runtime performance comparison was made.
