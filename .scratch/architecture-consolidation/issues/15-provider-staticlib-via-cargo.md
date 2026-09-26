# 15. Build the provider staticlib through Cargo

Status: ready-for-agent

`kiln-native-provider-ffi` is an `rlib` for the workspace, and the `.a` that emitted C links against
is a separate `cargo rustc --crate-type staticlib --features regexp,bigint,temporal` build run by
`scripts/package-native-number-provider.sh`. A provider ABI change can leave `cargo build` clean
while every native link fails.

Make the staticlib an ordinary Cargo artifact: a dedicated crate with `crate-type = ["staticlib"]`
that depends on the provider with the required features, so `mbx build` produces it and the
symbol-table check against `PROVIDER_EXPORTS` runs as that crate's test.

Coordinate with 06 step 4 (the same crates are being consolidated). `CLAUDE.md` still calls the
crate `kiln-native-number-ffi`; fix that name while deleting the section.

## Acceptance

- `mbx build` produces the archive the native link uses; the packaging script is deleted or
  reduced to a copy.
- The "cargo build does not build it" section of `CLAUDE.md` is deleted.
