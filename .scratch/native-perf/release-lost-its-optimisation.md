# `--profile native-release` stopped optimising, and is now 8-73x slower

**Role:** implementer. **State:** open, urgent. **Found:** 2026-09-07 on `7d34e611`.

Measured on `7d34e611`, warm median of 3, checksums matching node, against the same benchmarks on
`83747f26` (yesterday's last commit) where they were within 1.2x of node or ahead:

| benchmark | `83747f26` | `7d34e611` | node | was | now |
| --- | --- | --- | --- | --- | --- |
| cpu/loop | 27.9 ms | **1897 ms** | 26.0 ms | 1.07x | **73x** |
| non-io/class-poly | 1.51 ms | **34.0 ms** | 0.48 ms | 2.89x | **71x** |
| cpu/fib | 13.0 ms | **217 ms** | 22.0 ms | 0.57x | **9.9x** |
| non-io/object-spread | 0.09 ms | **2.88 ms** | 0.34 ms | 0.26x | **8.5x** |
| non-io/json | 28.8 ms | **43.5 ms** | 29.5 ms | 1.01x | 1.47x |

Two independent causes, both from the batch that landed today, and each alone is most of it.

## 1. Release emits the compact body, which has no specialization in it

`crates/kiln-native/src/lib.rs` sets, at four sites:

    emission.compact = options.profile != NativeProfile::Profile;

so every profile except `native-profile` is compact. Compact is not only shorter symbol names:
`crates/kiln-backend-c/src/lib.rs:1147` gates the fast clone on `!policy.compact`, and `compact`
is passed into `direct::DirectPlan::plan`, so a compact unit has **no fast clone, no versioned
body, no typed entry and no type twins**. The emitted C for `bench/cpu/loop.mjs` is 317 lines with
no `double` or `int64_t` declaration anywhere in it; every operation runs boxed through the
generic path.

## 2. Release compiles the module objects at `-O0`

Same file, in the release arm of the module compile options, on the reasoning that "the runtime
and prelude remain O3, where the hot generic operations live". They do not: a compiled function
body *is* the generated glue, and it is where every loop a program spends its time in ends up.
The build was under a second either way on these benchmarks.

## Why the obvious fix does not apply yet

Setting `emission.compact = false` for release does not compile:

    error: use of undeclared identifier 'kiln_module_path_..._fn_3'

`closed_world_function_liveness` (from "Prune unused native module functions") decides which
function bodies to omit, and its tests assert on `ku_fn_0(` / `ku_fn_1(` — the compact spelling.
Its liveness does not account for the references a specialized clone makes, so with the clones
back it omits a body that is still called. Allowing the fast clone under compact (dropping the
`!policy.compact` in the gate) compiles but changes nothing: the rest of the specialization is
keyed on `compact` too, through the direct plan.

So the three features interlock, and all three were written and tested with compact on.

## What it needs

A release profile that optimises: compact off for release, the closed-world liveness taught about
the clones, and the `-O0` dropped. If cheap cold builds of large bundles are wanted, they want
their own profile whose name says so — `native-release` is what `kiln build` gives a user by
default and what every benchmark and the `bench/results` baselines are measured with.

Nothing in the benchmark harness or the corpora catches this: every checksum still matches, the
programs are correct, they are just slow. `bench/results/baseline.json` is the file that would
have, and the gate does not run it.
