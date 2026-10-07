# String equality fast path validation

This candidate changes `kiln-string` equality only. `StringView` compares Latin1 and UTF-16 backing slices directly when their representations match; mixed representations still compare UTF-16 code units through the existing iterator. `StringUnits` delegates to that view comparison. No object identity, normalization, or scalar decoding is involved.

The Hash timeout profile recorded 131 combined leaf samples in `StringUnits` equality variants during the failing case. This source change is a targeted optimization candidate for that measured work. It does not claim to fix the full Hash deadline; the profile's sample share is only an upper bound, and no post-change Hash replay has been run.

## Validation

Source base: `efd9e68a8920d764ccc7df1f244d7591d74794d6`.

Command, using the repository-pinned Rust 1.97.1 and shared mbx cache/target:

```sh
env PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin \
  RUSTUP_TOOLCHAIN=1.97.1 \
  MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache \
  MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets \
  MBX_GC_AUTO=false MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
  ~/.local/share/mise/installs/mr-boxington/1.22.0/mbx test -p kiln-string --offline --jobs=1 -- --test-threads=1
```

Result: 8 unit tests passed, 0 failed; doc tests 0 passed, 0 failed. The added test checks equality across Latin1 and UTF-16 representations, non-ASCII Latin1, equal lone-surrogate units, replacement-character distinction, and unequal length/content. `cargo fmt --all` and `git diff --check` also passed.

A follow-up also adds a Latin1/ASCII fast path to `StringUnits == str`, which is the common property-key comparison route. Non-ASCII input and UTF-16-backed strings retain the exact UTF-16 iterator comparison. After this follow-up the same pinned command was rerun: 8 unit tests passed, 0 failed; doc tests 0/0. The final follow-up source digest is recorded in git at commit `e19cbd16` plus its immediate child.
