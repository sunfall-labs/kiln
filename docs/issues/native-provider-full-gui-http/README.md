# Full GUI provider HTTP capability

The provider contract advertises `ProviderVariant::FullGui` with `ProviderCapabilities::FULL_GUI`,
which includes HTTP. Unlike `Full`, the `full-gui` packager passes `--no-default-features`; its
feature string therefore must explicitly include both `realm` and `realm-http`. The previous
`FULL_GUI_FEATURES` omitted both, producing a manifest whose capability promise did not match the
compiled provider. The contract schema and provider ABI are unchanged; `src/lib.rs` is already in
the provider source digest list, so the corrected feature string changes source/archive identity
through the existing mechanism.

The new invariant is `tests::http_capability_variants_explicitly_enable_http_runtime`. It requires
every HTTP-capable non-`Full` variant to explicitly select `realm-http`, while checking the
`full-gui` no-default-features list for `realm` and `realm-http`. `Full` remains the documented
exception because it uses Cargo defaults, which include both features.

Red then green were run with the bounded command:

```sh
env MBX_GC_AUTO=false CARGO_BUILD_JOBS=1 KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- \
  mbx test -p kiln-native-provider-contract --lib http_capability_variants_explicitly_enable_http_runtime
```

The first run failed because `FullGui` claimed HTTP but its feature list lacked `realm-http`.
After changing the feature list, the same focused test passed. The full captured command output is
retained in [red.log](red.log) and [green.log](green.log). No provider archive was packaged or
rebuilt in this validation.
