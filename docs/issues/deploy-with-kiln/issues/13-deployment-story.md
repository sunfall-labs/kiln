# 13. The deployment story

Status: ready-for-agent

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

## Decisions, 2026-09-26

Decided by the project owner:

- **Toolchain: the host compiler is preferred.** `kiln build` uses the host C compiler when one is
  found. When none is found, **or** the host compiler cannot produce the requested target (static
  musl, or a cross-compile to another architecture), kiln downloads a pinned `zig cc` on demand and
  uses it. `--target linux-musl-arm64` therefore works on any machine.
- **First deploy artifact: static musl binaries for Linux x86_64 and arm64**, runnable
  `FROM scratch` or on distroless. CA roots and timezone data are embedded or declared, per ADR
  0067 §6. glibc dynamic builds remain whatever the host produces today and are not the target.

## Acceptance (implementation)

- `kiln build --target linux-musl-x86_64` and `--target linux-musl-arm64` produce static binaries on
  macOS and on Linux, with and without a host compiler installed.
- The downloaded `zig` is pinned by version and checksum, cached per machine outside `/tmp`, and
  never used when the host compiler can produce the target.
- A `FROM scratch` example image per tier 1 app in `examples/deploy/`, with its size beside
  `node:22-slim` and `oven/bun` in the compatibility page.
