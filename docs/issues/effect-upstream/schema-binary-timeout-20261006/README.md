# SchemaBinary zero-width array timeout replay

The original native-release Effect file was replayed without editing its tests, assertions, or per-case deadline.

- Source worktree: `~/Work/kiln-primitive-recovery`, production source commit `3faacb4c0dbfdc43b917c27eef73cf8efb1ca11a`
- Pinned test: `source/packages/effect/test/encoding/SchemaBinary.test.ts`, SHA-256 `61fc9333e7064cfc8216215204f0fbf932f0914f09ddfdbb78d9ce9d73c3bc1c`
- Frozen compiler CLI: `kiln-a0ab428cadabf39b7e9af894ddf102855e23f6d77698c8b76e89185c32420690`, SHA-256 equals its filename
- Provider manifest: ABI 82, SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`
- Selection: `encoding/SchemaBinary`, 193 registered cases, exact inventory/mapping, one worker and a 4 GiB process cap; original harness case deadline remained 5,000 ms.

Result: 192 passed, 1 failed, 0 skipped. The only failure is `SchemaBinary > layout and data errors > round-trips zero-width arrays across the allocation slack boundary`, which reports `TEST_TIMEOUT: 5000ms`. It checks `Schema.Null` and `Schema.Undefined` arrays at 1,048,576 and 1,048,577 entries, requiring a five-byte encoding and correct decoded endpoints.

The native build took 589.692 s: emit 26.414 s, compile 561.453 s, link 0.278 s. It produced 203,895,714 bytes of generated C, a 189,917,736-byte application object and a 130,649,400-byte stripped executable. The runtime object was a cache hit. The retained native executable is in the ignored private scratch output at `.scratch/schema-binary-replay-20261006/out/encoding/SchemaBinary/native.bin` (SHA-256 `9d084a5de6c00f71109b573bd41217d00f983c2e4f4a18d5c6b45ca4db1984e5`, ELF Build ID `e97d8c45c5b902d2450a65cc669fca710d6f9041`).

A second link with `KILN_NATIVE_KEEP_SYMBOLS=1` reused the same generated C and all application/prelude/runtime/launcher objects (0 s object compilation, 0.289 s link) and produced a debuggable binary for stack attribution. Its SHA-256 is `bcda894baa0516fc145c960aa2ed27a63b7b0ab5bf49f0d61955f1bc717bf6a4`, Build ID `b85801af0ea5aa5a238338a1c3ed3f87f11c166d`. This binary is diagnostic only; it was not used for the acceptance replay.

The full runner JSON, case log, build report, and outer log are retained under `evidence/`. No fix or speed claim is made here; the remaining work is to identify the dominant generated function/runtime kernel on the unchanged failing case.
