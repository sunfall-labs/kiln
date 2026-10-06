# Value-ABI computed invocation validation

This follow-up validates commit `3430d4633cc990d4d5184cd4e9a0357e34d222fb` in the owned
worktree, atop canonical-key fix `fab6ade351aac0a1e5268ea600befa11929cb145`. The native
`CallComputed` tail now passes its saved `kiln_val` callee, receiver, and contiguous fixed-arity
argument slice to the existing `kiln_c99_call_val` helper. Ordinary closures use its existing
direct value path; other callable forms retain its generic boundary fallback. The method name is
installed before invocation and cleared before throw/unwind handling.

The expanded fixture adds callable Proxy, bound function, and generator methods to exercise those
fallback paths. Node `v26.8.1` and the candidate interpreter output match byte-for-byte. Using the
candidate CLI, the unchanged tracked fixture matched in its original default native profile (1/1,
0 refused/diverged); a temporary copy with only `// kiln-native: profile=native-release` prepended
also matched (1/1); the tracked fixture matched under `KILN_NATIVE_GC_STRESS=1` (1/1, 0 skipped).
The tracked fixture has no profile directive.

The candidate CLI SHA-256 is
`205b8c5351dda1ca01ad6e32ba2450289277251aa75443a829d60c522de7c8d3`; it was built after commit
`3430d4633cc990d4d5184cd4e9a0357e34d222fb` with Rust 1.97.1 via:

```sh
RUSTUP_TOOLCHAIN=1.97.1 CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 MBX_GC_AUTO=false mbx build --jobs=1 -p kiln-cli --release
```

The fixture SHA-256 is
`84df3ee5f0353d0c87acd6f1aa1cf0ea623ada7a15f1787dbe8e7295849bcb57`. The saved Node and
interpreter output files are identical (SHA-256
`bc29beecfdc73c0e02ceb5b8c3fe8568c529e5c3cef0a506c5014c69ddd297fb`).

`native-default.log`, `native-release.log`, and `native-stress.log` retain the exact focused
runner summaries. `cargo fmt --all -- --check` and `git diff --check` pass. This is focused
correctness evidence, not a performance measurement or corpus result.
