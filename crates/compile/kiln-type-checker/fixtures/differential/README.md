# Type-checker differential fixtures

These fixtures preserve a small set of real project boundaries: package `exports`, inherited
JSONC config and path mappings, unchecked indexed access, and control-flow narrowing. The Rust
integration test consumes the complete source/config/package snapshot and checks completion,
diagnostic code plus exact byte span, and selected source facts. It intentionally does not compare
every emitted type or duplicate the larger checker unit suite.

The evidence discipline follows the useful part of Theo's `ts-rust` harness: each case has a
stable name and complete inputs, and a pinned compiler oracle is recorded alongside the checked
result. Reference audited at `pingdotgg/ts-rust` commit
`dfb3ca6d234ad05ac2519b46dcdba60bf4a0e50b` (MIT). The expected rows here come from the installed
TypeScript 7.0.2 CLI at
`~/.cache/kiln/type-engine-investigation/typescript-native-7.0.2/package/lib/tsc`.
No upstream syntax tree or checker implementation is used.

Run the Rust harness with `mise exec -- mbx test -p kiln-type-checker --test differential`.
Verify the pinned oracle with:

```sh
python3 crates/compile/kiln-type-checker/tools/verify_differential_oracle.py \
  --tsc ~/.cache/kiln/type-engine-investigation/typescript-native-7.0.2/package/lib/tsc
```

The CLI reports diagnostic start locations but does not expose end spans, so oracle rows record
code and line/column while the committed expected rows record exact source byte ranges. Updating
oracle evidence requires both `--update` and `--confirm-version 'Version 7.0.2'`; the verifier
prints the binary and fixture inventory hashes for review.
