# Default array-index definition fast path

The focused backend fixture was run against source base `10cdeb8f3397af8da6a24401f6d405817744c695` plus candidate commit `200a85467c26ca15582eadc71fdf97e77917978d` (fast path and fixture). The fast path keeps a complete all-true data descriptor on dense array storage; partial descriptors and any array with an overlay continue through the existing definition algorithm.

The test ran in the owned worktree `~/Work/kiln/.scratch/effect-array-define-fastpath`, using pinned Rust 1.97.1 and `mbx`, with `MBX_GC_AUTO=false`, one Cargo/mbx/Rayon worker and the shared cache/target roots. Exact command:

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:$PATH \
RUSTUP_TOOLCHAIN=1.97.1 \
MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache \
MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets \
MBX_GC_AUTO=false MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
~/.local/share/mise/installs/mr-boxington/1.22.0/mbx test -p kiln-backend-c \
array_create_data_property_keeps_default_indices_dense_under_gc_stress --offline -- \
--nocapture --test-threads=1
```

Authoritative tool session: `functions.exec` session `45325`, terminal exit code 0. One test passed, 330 were filtered; the separate emitted-C test binary had no selected tests. The fixture ran the C collector with stress collection every allocation while growing an array from inline storage to heap storage, preserving existing object/string values, holes, and the inserted tail value. It also checked the dense-storage invariant (`has_index_descriptors == false`) and own-key count.

`test.log` stores the captured terminal tail containing the test invocation, result, and cache summary. The complete initial dependency compiler stream was emitted across several tool polls and was not redirected to a durable file; this receipt does not claim to preserve that stream.
