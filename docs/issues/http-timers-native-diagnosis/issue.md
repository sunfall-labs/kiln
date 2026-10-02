# TypeScript native-link diagnosis

Date: 2026-09-13

## Reproduction

The same 9,947,645-byte bundle (`typescript.bundle.js`, SHA-256
`02fabc6de10d01cc38f66096ed8a5dbe675bf06c75e424e612e50df646fa3df4`) was run with the
candidate and clean baseline release CLIs. Both native runs reached the external Zig linker and
exited 1 with the identical failure:

```
kiln: native toolchain failed: native link failed (status Some(1)):
error: failed to resolve relocations and write atoms: Overflow
```

The standalone candidate log `typescript.log` was started before the final-gate rebuild. It
therefore contains the GC-policy candidate state, but its exact CLI binary hash at launch was not
captured and must not be identified with the later `ce1eff5e` release binary. The final gate later
reproduced the same TypeScript native-link exit 1 at `ce1eff5e`; that independent evidence is in
the gate's third-party log. The clean baseline was commit
`74f65cb1d6dc1fbe57e8588492776da99e407122`, with CLI SHA-256
`86ea699a98c9eba476aaa2c3d6a260dd02044a30854b4188e2a1ec73be5b0428` verified at launch; its
complete log is `baseline/typescript-absolute-2.log`.

The candidate process was active rather than hung: the observed chain was `kiln -> zig cc ->
zig clang`, with the clang child consuming about one CPU while resolving the generated objects.
The error names `libubsan_rt.a` and the generated `kiln_module_path_*.o` objects. The baseline
reproduces the same object names and relocation overflow, so this is not evidence of a GC or
runtime semantic failure and is not attributable to the GC-policy changes.

## Bounded follow-up

Keep this as a native toolchain/build-size issue. The next investigation should use existing
interfaces only:

1. Add a small diagnostic probe that records generated object sizes and the complete linker
   command when this error occurs, without changing the emitted program.
2. Check whether the supported Apple linker path can link the same object set, and whether the
   existing native-link command already has a configured linker-selection hook. If a fallback is
   supported, make it an explicit, reproducible toolchain option and test this bundle plus a small
   successful native bundle.
3. If no linker can accept the object set, evaluate splitting the generated translation unit or
   object groups at the existing module/emission boundary. Measure object and relocation sizes
   before changing code; do not introduce a compiler-wide rewrite for this one bundle.
4. Preserve this reproduction as a bounded regression fixture: success is a usable diagnostic or
   successful link, while a toolchain refusal must name the linker and generated object that
   exceeded its relocation limit.

No production source or linker configuration was changed for this diagnosis. The official
third-party corpus run remains the final-gate responsibility.
