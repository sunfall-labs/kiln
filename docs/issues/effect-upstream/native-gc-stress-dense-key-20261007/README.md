# Native GC-stress rooting subset — 2026-10-07

This is a targeted native-backend correctness check, not full native-corpus acceptance and not a timing run. The dense cycle-collector change is in the interpreter (`vm/cycle_gc.rs`); this native subset exercises the generated-C/runtime collector and allocation-rooting paths only. The dense interpreter collector is separately covered by its focused collector tests and the routine gate.

The executable `scripts/native.sh` accepted the seven explicit probe paths below. Its normal interpreter run served as the byte-and-exit-status oracle; `KILN_GC_STRESS=1` was set only for native runs. Terminal outcome: **7 matched, 0 refused, 0 diverged, 0 stress-skipped**. The supervisor exited 0 after 51.45 seconds. The native script retains its aggregate stdout; on successful comparisons it removes its private per-probe stdout files after comparing them, so this archive does not claim to contain those discarded files.

The frozen CLI is the routine-gate candidate built from source commit `edff3fc105558bcf74bac098d47dc4c889a8efb5` with SHA-256 `c61bb82a8afdd67f718bb3c3a2be7fe92b44b2c37d82b323c01b6ac82eabb948`. The provider manifest is ABI 82 / variant `full`, SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`. Probe source bytes match the frozen source checkout; individual SHA-256 values are in `raw/probe-inputs.json`.

| Probe | Rooting shape exercised |
| --- | --- |
| `weak-references-basic.js` | WeakRef and FinalizationRegistry target/token liveness, unregister behavior, synchronous allocation churn |
| `weak-references-job-rooting.cjs` | WeakRef target survival through nested CommonJS evaluation and allocation churn |
| `shared-weak.cjs` | WeakMap/WeakSet membership and weak reference to a shared graph |
| `shared-gc.cjs` | Shared graph edges across allocations and worker message channel setup |
| `json-shaped-build-edges.js` | Nested JSON construction, duplicate JSON object keys, stringify/parse round trips, deep containers |
| `json-deep-distinct-keys.js` | Distinct pending keys through nested JSON parser spill frames |
| `node-assert-cycles.cjs` | Cyclic object/array equality and two-node cycle handling |

The script ran from the repository root with `KILN_JOBS=1`, `RAYON_NUM_THREADS=1`, `KILN_NATIVE_GC_STRESS=1`, and a 900-second per-invocation infrastructure timeout. The systemd job applied a 4 GiB memory cap, 2 GiB swap cap, and a 2-hour outer bound. Root’s release build overlapped this correctness-only run, so no performance conclusion is drawn. The `scripts/native.sh` bytes match the frozen source checkout (SHA-256 `0fef47bb437f658cc03f4f8133a1b25496cbb269defcd9a0904c68990ab6c5d0`). The exact launch argv/environment, terminal status, service result, frozen CLI receipt, provider manifest, full script log, and checksums are under `raw/`; `manifest.json` hashes each archived raw file. The provider archive itself is not copied into this docs-only receipt; its immutable provider artifact is identified by the manifest digest and remains at the recorded external path.
