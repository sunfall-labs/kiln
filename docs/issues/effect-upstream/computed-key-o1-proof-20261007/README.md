# Computed-key O1 ranking source proof

This read-only proof grounds the ranking hint added in `ab622607` in the pinned generated-C
artifact, rather than assuming that a function ID or an upvalue index names a source binding.
Reproduce the extraction with:

```sh
python3 extract-proof.py
```

The script validates the `KILNART` record header, full-record SHA256, payload SHA256 and length,
module symbol, and pinned Effect source commit and `effect.ts` SHA256. It then extracts the
load/call and definition PCs, the entry closure vectors, the import-cell initialization, and the
raw frontend span metadata. The source checkout is the durable
`frozen-clock-native/workspace/source` at commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
expected generated-C cache record is
`f6/f6130aa23635332d913f66e077c4bf97deaa2d3639bf1f7912c090cb93129cd1`; its full-record SHA is
`b3c54f19e6f83bce9d3ff3337e2898731dc995e365e6054ba109c7576e02d473` and its payload SHA is
`40abce548e10301f3b35c1160673b475767e63009749b712f60d0fbbe0ee00fc`.

The compiled closure capture vectors establish the identity join: fn50 captures `locals[40]` at
upvalue index 4; fn123 captures the same `locals[40]` at upvalue index 1. Fn50 loads that upvalue
at PC 112, converts it to a property key at PC 113, and makes a computed call at PC 117. Fn123
loads its key from upvalue 1 at PC 11, converts it at PC 12, creates closure fn120, then defines
the named property at PC 14. The generated module initializer initializes `locals[40]` from
`imports[40]` and boxes that local as a cell. The pinned source's runtime import enumeration
identifies import 40 as `evaluate` from `./core.ts`; the same cell, not the same upvalue number, is
what joins the call and definition keys.

The C payload carries frontend byte spans but no per-function source-file path. The script prints
those spans as raw compiler metadata and separately verifies that the pinned source contains the
computed `evaluate` call. It does not treat frontend spans as offsets into authored TypeScript:
erasure and lowering can change coordinates, and a generated module can contain protos originating
in multiple files. The function/source relationship is inferred from generated body structure,
closure capture vectors, the module import table, and the pinned source—not from a span-to-source
slice. The ranking hint uses generated lexical-cell identity and does not change call dispatch.

## Backend validation

The planner tests and emitted-C integration target were run from repository HEAD
`6716e78879d37f8655d42b3d6dfa2ceba0d1f8a9`; `crates/native/kiln-backend-c/src/direct.rs` had
SHA256 `5955757668cd0d943ebdaef0187bc47e26e2de7d342feb354969dd7dccb1dbaa`. The environment used
Rust 1.97.1, the shared mbx cache, `CARGO_BUILD_JOBS=1`, `KILN_JOBS=1`,
`RAYON_NUM_THREADS=1`, and `--test-threads=1`.

The focused planner command was:

```sh
mbx test -p kiln-backend-c --offline loop_call_profile_tests -- --test-threads=1
```

It passed 6 tests (0 failed, 0 ignored); the other integration target had 37 filtered tests.
The meaningful emitted-C target was then run with:

```sh
mbx test -p kiln-backend-c --offline --test emitted_c -- --test-threads=1
```

It passed all 37 tests (0 failed, 0 ignored). Full command output is retained in
[`focused-loop-tests.log`](focused-loop-tests.log) and [`emitted-c-tests.log`](emitted-c-tests.log).
These checks validate planner behavior and C emission. They do not include a native AOT workload
or an Effect replay.

The actual generated Effect module was subsequently found to exceed the original profile's
independent 512-proto admission bound. Its retained generated C has SHA256
`8b3065777651a4d92c3a6197dedbcfb4db4cb0ee5871f85e62852380fd9203d5`, payload SHA256
`40abce548e10301f3b35c1160673b475767e63009749b712f60d0fbbe0ee00fc`, 739 proto indices (0–738),
and 20,004 instructions represented by frontend-function metadata. The emitted body still showed
fn120 as O0. The profile returned before computed-key analysis because of the proto-count bound.

Commit `948c52bb8bd03d83f1bdd3823fb8ef06294eac88` raises only that analysis admission cap to 1,024.
The 32,768-instruction, 256-loop-caller, 4,096-edge and depth-4 analysis limits remain; the O1
selector's separate 512-function bound remains unchanged. A focused regression builds the same
captured-key candidate graph with 739 protos and checks that it is admitted, then checks rejection
at 1,025. The focused suite passed 6/6; its complete log is
[`cap1024-focused-tests.log`](cap1024-focused-tests.log). This does not yet prove that the actual
Effect method is selected or that fn120's emitted attribute changes; the integrated AOT emission
must verify that separately.

After the cap change, the emitted-C integration target was rerun at source commit
`948c52bb8bd03d83f1bdd3823fb8ef06294eac88` (the tree also contained docs-only commits). It passed
37/37 tests, 0 failed/ignored, in 1.71 seconds; the target completed in 0.20 seconds. The tested
`direct.rs` SHA256 was `fdcf9415c0e8bb8fadc389af63db097966f99e23c5bfef3b2bf25e26b8dda559`. The pinned
Rust 1.97.1/mbx command was:

```sh
mbx test -p kiln-backend-c --offline --test emitted_c -- --test-threads=1
```

Its full output is retained in [`cap1024-emitted-c.log`](cap1024-emitted-c.log). This confirms the
existing emission integration suite remains green; actual Effect fn120 selection is still a
separate AOT check.
