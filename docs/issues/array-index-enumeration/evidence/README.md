# Array index enumeration VM validation

The candidate is source commit `25f269c852ebeabdf48517c0250693fd6e5944bd`, based on
`daf503a5`. Both test runs used that committed source, Rust 1.97.1 (`8bab26f4f`,
`RUSTUP_TOOLCHAIN=1.97.1`), offline dependencies, one `mbx` worker and one test thread. The
focused regression raw output is in [`focused-test.log`](focused-test.log) and reports 1 passed.
The full interpreter crate raw output is in [`interpreter-crate.log`](interpreter-crate.log) and
reports 278 passed, 0 failed, 1 ignored. Formatting and `git diff --check` passed.

Full-crate command (environment values shown separately to keep the command reproducible):

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin
RUSTUP_TOOLCHAIN=1.97.1
MBX_GC_AUTO=false
MBX_CACHE_DIR=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-cache
MBX_TARGET_ROOT=~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets
MBX_SCHEDULER_CPUS=3 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1
~/.local/share/mise/installs/mr-boxington/1.22.0/mbx test \
  -p kiln-interpreter --offline -- --test-threads=1
```

The focused test used the same environment with this command:

```sh
~/.local/share/mise/installs/mr-boxington/1.22.0/mbx test \
  -p kiln-interpreter --offline \
  object_keys_uses_array_index_descriptors_and_keeps_4294967295_named \
  -- --test-threads=1
```

Source SHA-256 values at the tested commit:

```text
5c283516ae4d1762c106fdd5db8112c3a367ad331def273b82a5032a7f225ada  crates/interp/kiln-interpreter/src/intrinsic_helpers.rs
4fb29d648927002559ee672fd393b140b9ca1cdd4e0964938c1c91f8b96fa9e5  crates/interp/kiln-interpreter/src/intrinsics/dispatch.rs
0c86373a085f2b290f8efb938a9aefd408fde1d5bea77c3ab1795cd116bc2cad  crates/interp/kiln-interpreter/src/tests/objects.rs
ace7b04ed8a24d731e407ffe95aa363aff3b5a9e8fd6bca699ce8ee4191e497c  docs/issues/array-index-enumeration/evidence/focused-test.log
3f93abd0bb61e0917be6b4f3739d9512d94db048e3f5b1ed789c050815ee1780  docs/issues/array-index-enumeration/evidence/interpreter-crate.log
```

An earlier focused-test attempt used the checkout's default `PATH` and omitted the pinned
`RUSTUP_TOOLCHAIN`; the parent identified its compiler process as Rust 1.99. That process was
interrupted (exit 130) before the test ran and is excluded from all results. Its complete raw
stream was not retained. The subsequent focused and full-crate runs above both used the pinned
Rust 1.97.1 environment.
