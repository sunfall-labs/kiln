Native recovery probe receipt
Date: 2026-10-06
Source worktree: ~/Work/kiln-primitive-recovery
Source commit: 3faacb4c0dbfdc43b917c27eef73cf8efb1ca11a
Frozen CLI: ~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/effect-recovery-combined-20261006/kiln-a0ab428cadabf39b7e9af894ddf102855e23f6d77698c8b76e89185c32420690
CLI SHA-256: a0ab428cadabf39b7e9af894ddf102855e23f6d77698c8b76e89185c32420690
Provider manifest: ~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/formatter/toolchain/provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/kiln-native-provider.json
Provider manifest SHA-256: 4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b

Results:
- dynamic-function-regexp-construction.js: manual native executable and Node both exit 0; identical stdout `dynamic regexp construction: 1 true`. Native executable SHA-256 b184d5753e62add6f1494633c2d2359c5636750a6b6a311182621f4acddfaf51.
- object-spread-sources.js + object-property-enumerability.cjs: scripts/native.sh 2 matched, 0 refused, 0 diverged.
- All three original probes with KILN_NATIVE_GC_STRESS=1: scripts/native.sh 3 matched, 0 refused, 0 diverged, 0 skipped.

Commands used KILN_JOBS=1 and RAYON_NUM_THREADS=1. Full stdout/stderr is retained in logs/.

Commands (from the detached source worktree):

```sh
KILN_NATIVE_PROVIDER_MANIFEST=<ABI82 manifest> KILN_NATIVE_NUMBER_PROVIDER_MANIFEST=<ABI82 manifest> KILN_JOBS=1 RAYON_NUM_THREADS=1 <frozen CLI> --one-shot build conformance/native/dynamic-function-regexp-construction.js --node --output .scratch/probe-evidence-20261006/bins/dynamic-function-regexp-construction.bin
.scratch/probe-evidence-20261006/bins/dynamic-function-regexp-construction.bin
node conformance/native/dynamic-function-regexp-construction.js
KILN_BIN=<frozen CLI> KILN_NATIVE_PROVIDER_MANIFEST=<ABI82 manifest> KILN_JOBS=1 RAYON_NUM_THREADS=1 scripts/native.sh conformance/native/object-spread-sources.js conformance/native/object-property-enumerability.cjs
KILN_BIN=<frozen CLI> KILN_NATIVE_PROVIDER_MANIFEST=<ABI82 manifest> KILN_JOBS=1 RAYON_NUM_THREADS=1 KILN_NATIVE_GC_STRESS=1 scripts/native.sh conformance/native/object-spread-sources.js conformance/native/object-property-enumerability.cjs conformance/native/dynamic-function-regexp-construction.js
```

The retained standalone build log reports generated C 24,240,078 bytes, app object 26,648,064 bytes, and executable 40,760,040 bytes. The frozen CLI and provider manifest hashes are recorded above. The standalone binary remains under the ignored `.scratch/probe-evidence-20261006/bins/` directory in the owned worktree.
