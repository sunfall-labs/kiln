# Changelog

Notable changes to kiln, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor bump may break things).

A release's notes are this file's section for that version: `.github/workflows/release.yml`
publishes the `## [x.y.z]` section matching the workspace version, so add one before tagging.

## [Unreleased]

## [0.1.0]

First public release.

- Compiles JavaScript and TypeScript (ESM and CommonJS) to native executables through C, with a
  bytecode interpreter that runs the same program without a build.
- Core `node:*` APIs (`http`, `net`, `fs`, `events`, `stream`, `buffer`, `path`, `url`, `util`)
  on a Tokio and Hyper runtime.
- Type-directed native code generation: checker facts drive guarded speculation, with a generic
  path behind every guard.
- Release archives for macOS (Apple silicon) and Linux (x86_64 and arm64, glibc).
- Security posture documented in `SECURITY.md`: programs run with the user's full authority (no
  sandbox, as in Node); `crypto` randomness always comes from the operating system; release
  executables are built with stack protection, FORTIFY and (on Linux) full RELRO; releases are
  gated on `cargo deny`.

[Unreleased]: https://github.com/sunfall-labs/kiln/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sunfall-labs/kiln/releases/tag/v0.1.0
