# Internal-key classification fast path

The Hash diagnostic profile recorded 66 leaf samples in `symbol::is_internal_key`. The fixed inventory contains 40 exact internal slots and four generated families, all prefixed by `@@`; ordinary property names were scanning these tables unnecessarily. The implementation now returns immediately for keys without that prefix, then preserves the existing exact-slot/family checks.

Focused regression coverage checks ordinary names, user-visible `@@observable` and `@@symbol-observable`, every exact internal slot, all four generated families, and incomplete near-prefixes. The repository's existing inventory test still ensures internal-slot literals remain classified.

Validation on source base `efd9e68a8920d764ccc7df1f244d7591d74794d6`, using pinned Rust 1.97.1 and shared mbx cache/target:

```sh
env PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin \
  RUSTUP_TOOLCHAIN=1.97.1 \
  MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache \
  MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets \
  MBX_GC_AUTO=false MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
  ~/.local/share/mise/installs/mr-boxington/1.22.0/mbx test -p kiln-interpreter --offline --jobs=1 internal_key_classification_skips_ordinary_names_without_widening_reserved_keys -- --test-threads=1
```

Result: 1 focused test passed, 0 failed; 282 tests were filtered. `cargo fmt --all -- --check` and `git diff --check` passed. No Hash replay or speedup claim is included.
