# Automatic interpreter cycle collection: focused validation

This candidate is based on `efd9e68a8920d764ccc7df1f244d7591d74794d6` in the owned
`namespace-erasure-autogc` worktree. The source-only candidate diff before this evidence directory
was added had SHA-256 `a6c5564080c47f5e8797939c0b57f994c7a9c4a00bafaac71778f6349b3fa4d0`.

The implementation counts object, array, and proxy census registrations per interpreter thread;
closure and generator registrations add pressure to their own `ModuleMap`, so collecting another
map cannot reset their trigger. The outermost VM run checks pressure after self-import namespace
refresh and every 1024 instructions. Collection uses the existing conservative Rc ownership
accounting. A pass resets only the current map's closure/generator pressure and the thread-wide
object census pressure; the next interval is the number of live graph nodes, with a 4096 minimum.
Nested VM runs defer automatic collection until the outer run resumes. WeakRef job keepalive is
unchanged.

## Validation

All commands used Rust 1.97.1, `mbx`, one Cargo/KILN/Rayon worker, one test thread, the shared
integration cache/target, `MBX_GC_AUTO=false`, and `--offline`:

```sh
export PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin
export RUSTUP_TOOLCHAIN=1.97.1
export MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache
export MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets
export MBX_GC_AUTO=false MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1
MBX=~/.local/share/mise/installs/mr-boxington/1.22.0/mbx
$MBX test -p kiln-interpreter --offline --jobs=1 automatic_collection_is_limited_to_the_outermost_vm_run -- --nocapture --test-threads=1
$MBX test -p kiln-interpreter --offline --jobs=1 automatic_cycle_collection_respects_vm_roots_and_job_keepalive -- --nocapture --test-threads=1
$MBX test -p kiln-interpreter --offline --jobs=1 -- --test-threads=1
```

The first focused test passed 1/1 and checks that a nested VM run is not treated as an outer safe
point. The lifecycle test passed 1/1 and drives collection at real opcode boundaries: an active VM
local and same-job WeakRef keepalive preserve their targets; after simulated job-keepalive release,
the unrooted target is reclaimed; after the active local is cleared, its self-cycle is reclaimed.
It also verifies that pressure accumulated in an independent `ModuleMap` remains untouched.

The full interpreter crate passed 283 tests, failed 0, ignored 1; doc tests passed 0/0. Complete
raw logs and checksums:

| Log | SHA-256 |
| --- | --- |
| `focused-guard.log` | `3bb2ff395ae9b7261d3422055d31adb5fb21230675151fa56302867852d16642` |
| `focused-lifecycle.log` | `bbd5910d5f83863b2f75b198a926de0bf7b0515746698f678789a8644414b6f1` |
| `interpreter-crate.log` | `8eb985ac1c75c29f5ce2f851129eaee79d8c9e4594d8a8bc108a864d3555a333` |

## Limits

This validates the collector trigger and conservative-root controls, not memory reclamation on the
original 178-case Arbitrary suite. Nested VM callback loops defer automatic collection until the
outer run resumes, and a single native opcode can still allocate many values before that boundary.
The earlier Arbitrary run ended after 172 recorded cases with a 16 MiB allocation failure under a
4 GiB cap; no rerun with this candidate has been made. Do not treat the memory failure as solved
until all 178 unchanged cases complete under the original limits.
