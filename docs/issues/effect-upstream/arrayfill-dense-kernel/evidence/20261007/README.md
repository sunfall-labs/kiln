# Guarded dense `Array.prototype.fill` candidate

This receipt records the interpreter-crate check for the owned candidate. The focused differential
fixture is present in the candidate source but has **not** been executed, so this is not evidence
that its Node parity cases pass.

## Candidate

- Worktree: `~/Work/kiln/.scratch/effect-arrayfill-dense-kernel`
- Branch: `fix/interpreter-arrayfill-dense-kernel`
- Base: `edff3fc105558bcf74bac098d47dc4c889a8efb5`
- Source commit: `96aaf9399a16e19e40be026e4cb0b597ca47c0f2`
- Candidate files: `crates/interp/kiln-interpreter/src/vm/array_like.rs`,
  `crates/interp/kiln-interpreter/src/vm/array_like_ops.rs`,
  `crates/interp/kiln-interpreter/src/vm/object.rs`, and
  `conformance/differential/array-fill-dense-range.js`

The fast path runs only after the observable start/end conversions. It fills a range already
inside dense storage, and falls back for exotic element descriptors, nonextensible/frozen arrays,
explicit receiver prototypes, custom indexed intrinsic chains, and ranges beyond the existing
walk limit. The ordinary per-index path was also corrected to send holes through indexed `Set`,
including prototype descriptor checks. The fixture covers inherited setters on receiver and
intrinsic prototype chains, prototype mutation during end coercion, nonextensible holes, partial
writes before inherited refusal, and a 262,144-element dense range.

## Validation performed

Command (pinned Rust 1.97.1, one Cargo job, one test thread):

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin \
RUSTUP_TOOLCHAIN=1.97.1 \
MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache \
MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets \
MBX_GC_AUTO=false MBX_SCHEDULER_CPUS=6 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
mbx test -p kiln-interpreter --offline -- --test-threads=1
```

Result: exit 0, 290 passed, 0 failed, 1 ignored; doc tests 0. Full raw output:
`interpreter-test.log` in this directory. SHA-256:
`2d500c26be35e1b0f2b03a12039e38a32b7e4b473d62dbe7eb07b04343d1ae58`.

This crate result does not validate the new JavaScript fixture. The focused Test262 subtree for
the later frozen candidate CLI is `built-ins/Array/prototype/fill` (22 files in the pinned local
corpus). Run it with the repository runner and original budgets, for example:

```sh
KILN_BIN=<frozen-candidate-cli> scripts/test262.sh \
  --out <unique-output-path> --path built-ins/Array/prototype/fill
```

For the differential fixture, use the repository's `scripts/differential.sh` with
`KILN_BIN=<frozen-candidate-cli>` and `KILN_DIFFERENTIAL_DIR` pointing to an isolated directory
containing this fixture alone. That isolates the new probe without representing a full differential
corpus run.

## Digests

```text
crates/interp/kiln-interpreter/src/vm/array_like.rs       f57c5046a02eb92b65e244d55d7c098c4d6ec7a288abc1bf77c7c3bb81d24647
crates/interp/kiln-interpreter/src/vm/array_like_ops.rs   e5ef10f65fe1c56dd9772c5db2c5883e465c5b8bb8de2975f7c15447d0279544
crates/interp/kiln-interpreter/src/vm/object.rs          888387e23f2baa6412d7938855c1db06c42cdfe59ef4978974564f132098e602
conformance/differential/array-fill-dense-range.js       6c38214dd8cb266a743604abe5c832d7561ee8df6668b7751b64927250044fac
```
