# Native string-key hash width candidate

This receipt records correctness and artifact provenance for the width-specialized
`kiln_c99_key_hash` implementation. It does not establish a performance improvement.

## Source and artifacts

- Source commit: `62f641c4c0763add47b58eecabd4c766b4e68577`
- Changed runtime source SHA-256: `9f69c70f1cf6ad8a1080400e683a72c4cac70ea065b4811244a85c3b617f047a`
- Focused runtime fixture SHA-256: `2e7bfa941f46944348996040bb80053b34de43cf345a5a431ddfd13343d4b18c`
- CLI SHA-256: `7fbcf50e909aded0f96f62987fd6e45881410efb144f6742d0a40471f1f273a5`
- Candidate Hash executable SHA-256: `ad03fe0717a3b12076c7b946000d96ac22e0773ad43ae0c9d815116caa3152e6`
- Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`
- Provider archive: `libkiln_native_provider_ffi.a`, 32,638,290 bytes.
- Native profile: `native-release`; runtime and compile workers: one; heap cap: 4096 MiB.

The original Hash source and test inventory were unchanged. The candidate replay used the
pinned Effect workspace and explicit ABI-82 provider manifest. Its candidate CLI and output are
under `.scratch/hash-width-candidate/out/` in the owning worktree; binaries are intentionally not
committed.

## Results

The unchanged Hash file completed **24/25** cases. The sole failure is the same original 5,000 ms
timeout seen before this kernel change:

`Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`

The other 24 cases passed. All 25 cases were mapped to the Node/Vitest reference; there were no
inventory, mode, or mapping mismatches. The timeout was not extended.

Five focused key/property probes passed in ordinary native parity and again with
`KILN_NATIVE_GC_STRESS=1` (10 matched executions total):

- `computed-key-to-property-key.js`
- `dictionary-key-lookup-semantics.js`
- `shared-static-key-domains.js`
- `symbol-keyed-inline-caches.js`
- `string-units-json-keys.js`

The backend C library suite had already passed 340 tests, with 0 failures and 3 ignored. Its
focused width-reference fixture compares Latin-1 and UTF-16 representations against the previous
unit-at-a-time hash for lengths 0–67, all key-domain flag combinations, deterministic randomized
inputs, equivalent Latin-1/UTF-16 spellings, lone surrogates, and null inputs.

## Build and binary verification

The candidate Hash native build reported 184,074,916 bytes of generated C and a 167,284,024-byte
application object, both built. The 2,815,248-byte prelude object was built; the 16,288-byte
runtime object and 1,584,584-byte launcher object were cache hits. This identifies the edited
`runtime/heap.c` implementation as part of the rebuilt prelude object, rather than the separately
cached runtime object. Emission took 12.713 s, C compilation 179.727 s, and linking 0.227 s. The
full build took 194.914 s in the native build log (197.341 s in the runner envelope). Other bounded
jobs were active, so these durations are diagnostic and not isolated timing measurements.

A subsequent `KILN_NATIVE_KEEP_SYMBOLS=1` relink reused generated C and every object (0 compile
seconds; 0.271 s link). The relink's `.text` section is byte-for-byte identical to the candidate
Hash executable: both are 95,784,707 bytes with SHA-256
`05c3443dc946ea32e85e9530661307b8bf9487c8f5d551e16c94b1bc97acf55a`. The symbolized relink
disassembly confirms one string-width dispatch before the loops, with separate Latin-1 and UTF-16
loop bodies; it no longer tests the width flag for each loaded unit. The relink is used only for
that code inspection, not as a second acceptance run.

The Hash workload still reaches its original timeout, so this change has not cleared that
correctness/liveness result. No speedup is claimed from the concurrent build or these correctness
runs; a quiet paired performance comparison remains necessary to quantify the kernel's effect.
