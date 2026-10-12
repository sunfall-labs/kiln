# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's **Report a vulnerability** button
on this repository's **Security** tab, rather than in a public issue. Include the kiln version
(`kiln --version`), the platform, and the smallest program that shows the problem.

You can expect an acknowledgement within a few days. A fix is released as a new patch version,
and the advisory names the versions it affects once the fix is available.

## Supported versions

kiln is pre-1.0. Security fixes land on the newest `0.x` release only; please upgrade before
reporting a problem in an older one.

## What a kiln program can do

kiln does not sandbox the programs it runs. Like Node, a program run with `kiln run`, and every
native executable `kiln build` produces, has the full authority of the user who runs it:

- it can read and write any file that user can, and read every environment variable;
- it can open network connections and listen on ports;
- it can start child processes (`node:child_process` `spawn`/`spawnSync`) and, through
  `node:ffi`, call native code;
- it can evaluate strings as code with `eval` and `new Function`.

There is no permission model in this release. Run code you do not trust inside an operating
system sandbox (a container, a separate user, seccomp, a VM), exactly as you would with Node.

The language-level boundaries kiln does keep:

- **Realms are isolated.** Workers and actors share no mutable objects; values cross between
  them by structured clone, ownership transfer, or as sealed immutable graphs (`kiln:shared`,
  ADR 0073). The one shared mutable memory is a `SharedArrayBuffer`'s bytes, as in Node.
- **Randomness.** `crypto.getRandomValues`, `crypto.randomUUID` and `node:crypto`'s
  `randomBytes`, `randomUUID`, `getRandomValues` and `randomFillSync` always draw from the
  operating system's CSPRNG. `Math.random` is not a cryptographic source, as in Node: it is
  seeded from the operating system once per run, and is reproducible only under
  `kiln run --clock virtual`.

## Hardening of what kiln ships

- **Native executables** built with the release profiles (the `kiln build` default) are compiled
  with `-fstack-protector-strong` and `-D_FORTIFY_SOURCE=2`, and on Linux linked as
  position-independent executables with full RELRO (`-z relro -z now`) and a non-executable
  stack.
- **Release archives** are built by `.github/workflows/release.yml` from a tagged commit, with
  every action pinned to a commit SHA, and only after `cargo deny` has checked the dependency
  tree against `deny.toml`: no known advisory without a recorded reason, no licence outside the
  allow list, and no source other than crates.io. Each archive is published with a SHA-256
  checksum.

## Known limitations

- kiln's native runtime includes a large hand-written C runtime and Rust code with `unsafe`
  blocks at the boundary between them. They are tested by the conformance corpora, by collector
  stress runs and by a differential fuzzer (`fuzz/`), but they have not had an external audit.
- Release archives are not yet signed and carry no provenance attestation; verify them against
  the published checksums.
