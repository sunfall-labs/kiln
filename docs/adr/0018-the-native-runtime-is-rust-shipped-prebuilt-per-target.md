# ADR 0018: The native runtime is Rust, shipped prebuilt per target

## Status

**Superseded by ADR 0028.** The linked Rust semantic provider proposed here proved the target archive,
manifest and private-C-ABI path and is now a migration slice of the process-wide Tokio-backed Rust
runtime accepted there. This document remains as the narrower proposal and historical rationale;
its unresolved acceptance gates are not active decisions.

When this was proposed, ADRs 0009, 0013 and 0014 remained normative until the target matrix,
internal C ABI, allocator and panic policy, archive manifest and missing-archive fallback were
decided. Those were this proposal's acceptance gates. ADR 0028 makes the broader runtime decision
and states which earlier decisions it refines or preserves; this document no longer gates them.

## Context

design.md §14 item 9 is "Rust runtime with stable HostOps ABI", and §6 draws the same thing: JS
built-ins over a stable HostOps ABI over a "Rust runtime/provider". That is not what exists.
`kiln-native-runtime` is a Rust crate that emits C source as string constants, and
`kiln-native-number` vendors `ffc.h` plus a C adapter. The emitted artifact contains no Rust.

ADR 0014 gives the reason: conversion happens "inside emitted executables and shared libraries
after Kiln's Rust process exits", so "a Rust-only crate therefore cannot be the target runtime".

That single inference does not hold. A Rust staticlib linked into the executable runs perfectly
well after the compiler process exits; the emitted binary does not require the compiler to be
alive. The real constraint is one step to the side and is a good one: building an artifact for
target T with Rust in it requires `rustc` and `rust-std` for T on the machine doing the build,
where the C path needs only whatever `cc` is present. ADR 0009 deliberately kept even Zig optional,
and a hard Rust toolchain requirement on every user's machine would be a larger version of the
dependency it declined. Everything else in 0013 and 0014 stands.

The motivation is that runtime primitives are implemented twice. ADR 0014 states it plainly: the C
adapter and `ffc` serve the artifact, while "the Rust interpreter exposes the same language
operation from `kiln-host-abi`, with its own grammar/radix implementation and bit-exact shared
vectors". Two implementations of one language operation, held in agreement by differential vectors,
once per primitive rather than once in total.

Regular expressions, `BigInt` and Intl are the reason this matters at scale — `regress` was adopted
because it "targets ECMAScript semantics specifically" and matches over UTF-16, and no C
counterpart will agree with it on backreferences, lookaround and `lastIndex` without becoming a
maintained project of its own. **They are motivation, not scope.** A compiled regular expression is
persistent allocated state with a handle, an owner and a disposal point, and a `BigInt` is an
allocated value; neither is expressible over a seam that carries bytes and doubles. They belong to
a later native heap and ownership ADR and are not enabled by anything decided here.

## Decision

**The native runtime is Rust, and the first migration replaces only the number provider.**
`kiln-native-runtime` continues to generate the C CLI host and the launcher; that is a separate
concern from the semantic primitives and is not touched. A new `no_std` semantic-core crate holds
the language operation, and a thin `extern "C"` adapter exposes it. The seam already exists and
does not change: the C prelude declares `kiln_number_to_js_string` and `kiln_string_to_number` as
an "internal link-time runtime contract" supplied by a separate object and explicitly not part of
the public embedding ABI. `kiln_value` and `kiln_host` remain the embedding ABI, so an embedder
cannot observe the change.

**The first migration is `no_std` with no `alloc`, over caller-supplied buffers.** The existing
signature `int kiln_number_to_js_string(double, char *buffer, size_t capacity)` is already
allocation-free and caller-owned, and the core is written to that shape. This is chosen precisely
to avoid deciding the allocator question prematurely: with no allocation there is no
`#[global_allocator]`, no infallible-allocation failure mode, and no ownership contract to design.

**The core must be panic-free by construction, not panic-abort by configuration.** `panic = "abort"`
prevents unwinding across `extern "C"`, but it makes any panic or allocation failure fatal to the
*host* process — and `build_shared_library` exists, so kiln output is already linked into processes
kiln does not own. Killing an embedder's process is not an acceptable failure mode for a number
formatter. The core therefore uses checked arithmetic and bounds-checked access, denies `unwrap`,
`expect`, indexing panics and arithmetic overflow by lint, and returns the existing negative error
codes for every failure. The `#[panic_handler]` a `staticlib` requires is then an unreachable
backstop rather than a policy, and its presence should be verified by a no-panic link check rather
than asserted.

**Archives are built in CI per target and shipped with the compiler, described by a manifest.** A
read-only cache layer is storage; it does not select, package or validate a raw target archive. The
shipped manifest carries the full Rust target triple, the toolchain and source identity, the PIC
and linkage mode, the deployment and libc baseline, the ABI version, and the archive's payload
digest. Those fields participate in both the artifact cache key and the final link key, so an
archive cannot be silently substituted for one built differently.

**Both linkage modes are supported, as design.md §6 and §8 already specify.** §6 states that "the
runtime can be a dynamic library during development" and that "release builds can link the same
runtime statically"; §8's `native-dev` profile says "dynamic runtime". The first draft of this ADR
specified static archives only, which was a deviation from the design rather than a realization of
it. `native-dev` links a dynamic runtime and `native-release` links the archive. Because the
dynamic case arrives first, a shared runtime needs an explicit export list or version script from
the outset: a Rust staticlib bundles its upstream dependencies, may retain system-library
dependencies, and otherwise exports far more symbols than the contract names.

**Dead stripping is platform-specific.** `--gc-sections` is ELF and LLD; Mach-O needs
`-dead_strip`; Windows needs a policy that does not exist yet. The first draft named only the first
of these. The property being bought — a program that never constructs a `RegExp` does not link a
regular expression engine — is worth having on each platform, but it is three configurations, not
one flag.

**Missing-archive policy: shipped archive first, then a source build only when the required Rust
target is installed, otherwise a deterministic error naming the supported targets.** This was the
open question in the first draft and Codex's answer resolves it. Note that Rust organizes support
by full target triple rather than by architecture, so `no_std` reduces libc coupling but does not
collapse `-gnu` and `-musl` into one archive; the matrix is a list of triples.

## What must be decided before acceptance

1. **The target matrix.** Which triples ship, and what the deployment and libc baseline is for each.
2. **The internal C ABI for the semantic core.** Buffer ownership, error codes, and the versioning
   rule for adding an operation. It is internal, not the embedding ABI, and should say so.
3. **Allocator and ownership policy.** Not needed for the number provider, required before regex or
   `BigInt`. This is the heap and ownership ADR that gates everything beyond scalars. It must decide
   one thing early that is expensive to change later: variable-length payloads — string bytes, array
   backing stores — must not be `malloc`-backed Rust allocations freed from a finalizer. CRuby
   measured *negative* scaling with thread count when freeing many small allocations, across glibc,
   jemalloc and tcmalloc alike, and had to move those buffers into the collected heap. Decide that
   before the first payload type exists.
4. **The archive manifest schema**, and which of its fields enter the cache key versus the link key.
5. **Export-list policy for shared runtime builds**, since `native-dev` needs one immediately.
6. **Finalizer policy across the Rust boundary.** The moment a collected `kiln_value` owns a Rust
   resource, `Drop` and garbage collection meet, and that intersection is not routine: the one
   published attempt to reuse Rust `Drop` implementations as GC finalizers needs three distinct
   static analyses to do it soundly — rejecting unsafe destructors, eliding unnecessary ones, and
   preventing premature finalization. This ADR proposes exactly the runtime in which that problem
   arises, so it cannot be deferred to the collector's ADR without at least deciding whether
   collected values are permitted to own Rust resources at all. The cheapest answer, and the one the
   scalar-only first migration is consistent with, is that they are not.

## Consequences

- One implementation of ECMAScript number conversion, shared by the interpreter and the artifact.
  The vendored `ffc.h` and the C adapter become removable, and the differential vectors that hold
  the two implementations equal become a regression check on one implementation rather than the
  thing preventing divergence. That is a correctness argument before it is a simplification one.
- Moving `kiln-host-abi` or its callers onto a `no_std` core is an extraction and a port, not a
  feature flip. `regress`, `num-bigint` and `num-traits` are currently enabled with `std`, and
  `kiln-host-abi` itself uses `String`, `Display` and `std::error::Error`. The first draft claimed
  otherwise and was wrong. Only the semantic core needs to move; the interpreter-side crates can
  keep `std` and depend on the core.
- kiln acquires a target support matrix. Today any machine with any `cc` can build; afterwards, a
  target is supported when an archive exists, when a Rust target is installed, or not at all. The
  fallback ladder above keeps the last case rare and makes it explicit rather than mysterious.
- The archive is a distribution artifact with an identity that must be checked, which is new
  surface. Getting the manifest wrong produces a silently mismatched link rather than a build
  error, which is the failure mode the digest fields exist to prevent.
- Nothing here enables regular expressions, `BigInt` or Intl in the artifact. They motivated this
  and remain blocked on the ownership ADR.
- design.md §14 item 9 and §6 are realized rather than deviated from. This ADR argues that the
  current C runtime is the departure and that it should end, which is the opposite direction from
  the usual ADR here.

## Sources

Checked 29 August 2026, for the two claims above that come from outside this repository.

- Finalizers where Rust `Drop` meets a collector: Hughes & Tratt, "Garbage Collection for Rust: The
  Finalizer Frontier", <https://arxiv.org/abs/2504.01841> (revised September 2025).
- Parallel-free scaling and the move of variable-length payloads into the collected heap: Wang,
  Blackburn, Zhu & Valentine-House, "Reworking Memory Management in CRuby: A Practitioner Report",
  ISMM 2025. <https://doi.org/10.1145/3735950.3735960>
