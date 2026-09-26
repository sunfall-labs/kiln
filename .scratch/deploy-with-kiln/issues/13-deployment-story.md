# 13. The deployment story

Status: ready-for-human

The promise is "ship a small binary", so shipping has to be easy. Decide and document:

- **Toolchain.** `kiln build` needs a C compiler. Options: require one (documented per platform);
  bundle one (for example `zig cc`); or publish a build container image with everything in it.
- **Static binaries.** A musl-linked Linux binary that runs `FROM scratch`, and what that costs
  (TLS roots, timezone data, DNS resolution).
- **Cross-compilation.** Build on macOS for Linux x86_64 and arm64, the common case for deploys.
- **Examples.** A `Dockerfile` that keeps `npm ci` for dependencies and uses `kiln build` for the
  artifact; the image size next to `node:22-slim` and `oven/bun`.
- **Build time.** A cold native build of each tier 1 app, measured and published beside the
  runtime numbers, because it is part of what a user experiences.

## Acceptance

- Decisions recorded; one working `Dockerfile` example per tier 1 app in `examples/deploy/`.
- Image size and cold build time in the compatibility page.
