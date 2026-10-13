# Changelog

Notable changes to kiln, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor bump may break things).

A release's notes are this file's section for that version: `.github/workflows/release.yml`
publishes the `## [x.y.z]` section matching the workspace version, so add one before tagging.

## [Unreleased]

Next: Linux on arm64 (48-bit pointers in the value layout).

## [0.1.1]

- Native string accumulation now grows private loop-carried buffers geometrically, avoiding
  repeated copies of an HTML renderer's entire output prefix.
- Native replacement builds results from borrowed spans, reducing temporary allocations.
  Replacement coercion rejects Symbols and propagates exceptions even when no match is found;
  empty Unicode RegExp matches advance over complete surrogate pairs.
- `String.repeat` preserves lone UTF-16 surrogates in native programs, including pairs formed
  across repetition boundaries.
- Builtin type facts survive TypeScript lowering; stream hot paths retain object shapes and
  avoid unnecessary state capture.
- Added bounded fuzz campaigns for JSON parsing, HTTP framing and shared-graph input.
- Added `@sunfall/kiln` and an exact-version Linux binary package for npx and bunx. Platform
  packages include every native provider library and need no install scripts or downloads.

### Release availability

- Linux x86_64 (glibc): built manually on Ubuntu 22.04, with checksums and unpacked smoke tests.
- macOS arm64: pending. Use the v0.1.0 archive until a fresh macOS build is available.
- This manual release has no GitHub Actions build-provenance attestation. GitHub-hosted jobs
  were blocked by account billing; the source tag and SHA256SUMS identify this release.

## [0.1.0]

First public release.

- Compiles JavaScript and TypeScript (ESM and CommonJS) to native executables through C, with a
  bytecode interpreter that runs the same program without a build.
- Core `node:*` APIs (`http`, `net`, `fs`, `events`, `stream`, `buffer`, `path`, `url`, `util`)
  on a Tokio and Hyper runtime.
- Type-directed native code generation: checker facts drive guarded speculation, with a generic
  path behind every guard.
- Release archives for macOS (Apple silicon) and Linux (x86_64, glibc).

### Known limitations

- Linux on arm64 has no release: native programs crash at startup there, because the NaN-boxed
  value layout keeps 47-bit pointers and arm64 Linux maps programs and memory above 2^47. It is
  the next platform (docs/issues/arm64-linux-48-bit-pointers.md).
- Security posture documented in `SECURITY.md`: programs run with the user's full authority (no
  sandbox, as in Node); `crypto` randomness always comes from the operating system; release
  executables are built with stack protection, FORTIFY and (on Linux) full RELRO; releases are
  gated on `cargo deny`.

[Unreleased]: https://github.com/sunfall-labs/kiln/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/sunfall-labs/kiln/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/sunfall-labs/kiln/releases/tag/v0.1.0
