# 01. A native link fails when SDKROOT names the Xcode 27 SDK

Status: needs-triage
Feature: toolchain-sdkroot

Found 2026-10-02 while landing the release-packaging branch: `git push` ran the pre-push hook,
and the HTTP close contract failed every native link with

```
ld: multiple errors: tapi error: malformed file
  .../MacOSX.sdk/usr/lib/libSystem.B.tbd:4:20: error: unknown architecture
```

Column 20 of line 4 is `arm64e.x1-macos` in the SDK's target list, which Xcode 27's `ld`
(ld-27037.1) reads and the Command Line Tools 26.6 `ld` does not. So some link picked the older
linker.

What is established:

- The trigger is `SDKROOT=/Applications/Xcode.app/.../MacOSX.sdk`, which Apple's `/usr/bin/git`
  shim exports to hooks. `git -c alias.x='!bash scripts/check-http-close-contract.sh' x` fails;
  the same with `env -u SDKROOT` passes, and so does running the script from a shell.
- `SDKROOT=<that> /usr/bin/clang a.c` alone links fine and `clang -###` names Xcode's `ld`, so
  plain clang is not enough to reproduce. The failing command was kiln's own link
  (`kiln-native`, `/usr/bin/clang -Wl,-stack_size,0x4000000 ... -lm -o program`); the first push
  also failed rustc's links of build scripts under mbx.
- Not established: why the linker choice changes. A suspect is the environment kiln and mbx pass to
  child processes -- if either clears most of the environment but keeps `SDKROOT`, the `clang`
  shim may resolve a different developer directory than an interactive shell does.

Workaround in place: `.githooks/pre-push` and `.githooks/pre-commit` unset `SDKROOT`.

Why it matters beyond the hook: a user whose environment sets `SDKROOT` (an Xcode build phase, a
shell profile, a CI image) would see `kiln build` fail the same way. Reproduce with
`SDKROOT=$(xcrun --show-sdk-path) kiln --one-shot build hello.ts` on a machine with both Xcode 27
and an older Command Line Tools, and compare the child environment of the link.
