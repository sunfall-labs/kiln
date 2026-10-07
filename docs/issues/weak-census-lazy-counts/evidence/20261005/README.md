# Lazy weak-census count snapshots

Candidate source is based on `f16774178086e86a79de5a7ca7cf972325f5f745`. The source diff SHA-256
before commit was `263601af5964a62c84f4c93fbbf2179170e39c311d125c497626484f8b82242f`.

`WeakCensus` now stores only its weak-entry vector and unchanged prune threshold. Registration still
adds exactly one weak handle; pruning still removes expired handles at the same threshold. The
actor-transfer proof builds its numeric pointer multiplicity map from the current weak-entry
vector using `Weak::as_ptr()`, without upgrading or cloning weak handles. Thus repeated census
registrations remain counted, and a caller's separate `Weak` still prevents an exclusive move.

The full `kiln-interpreter` crate suite passed: 281 passed, 0 failed, 1 ignored. The raw output is
in [`interpreter-crate.log`](interpreter-crate.log), SHA-256
`8e258cbe4bf743da56f9109e3e29976bc23027e2d4b6d6e6fcc625101e8b0cb3`. It includes both
`gc_census::tests::derived_counts_preserve_duplicate_weak_edges_through_pruning` and
`vm::workers::ownership_tests::allocation_census_weak_edges_do_not_masquerade_as_sender_aliases`.
The latter proves census-owned Weak handles are discounted for actor transfer, while a distinct
user Weak handle still refuses it. Formatting and `git diff --check` passed.

Command and environment:

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin
RUSTUP_TOOLCHAIN=1.97.1 MBX_GC_AUTO=false
MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache
MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets
MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1
mbx test -p kiln-interpreter --offline -- --test-threads=1
```
