# Linux on arm64: the value layout needs 48-bit pointers

Status: open — the next platform after 0.1.0

Every native program crashes at startup on Linux arm64 (`aarch64-unknown-linux-gnu`), including
an empty one; the interpreter is unaffected. macOS on Apple silicon and Linux x86_64 are fine.

## Cause

ADR 0053's NaN-boxed `kiln_val` keeps a **47-bit** pointer payload
(`KILN_PAYLOAD_MASK = 0x00007FFFFFFFFFFF`, `KILN_VAL_PTR` in `runtime/prelude_types.h`), and bit
47 separates tags: Temporal is `0xFFFE8…`, shared facades `0xFFFF8…`. That holds where user space
sits below 2^47 — Linux x86_64 always, macOS arm64 in practice. Linux arm64 loads a PIE at
`0xaaaa_aaaa_0000` and maps memory near `0xffff_…`, both with bit 47 set, so the first pointer
into the program's own data loses its top bit when unboxed.

Observed on GitHub's `ubuntu-22.04-arm` runner (clang 14; gcc 11 crashes identically, so it is
not a compiler issue), in `@kiln/runtime-bootstrap.ts`'s module body:

```
x1 = 0xaaaaab505890                 a pointer into the program's data
x9 = 0x2aaaab505890                 the same pointer after KILN_VAL_PTR
=> str x8, [x9]                     SIGSEGV
```

## Fix options

1. **48-bit payload** (the real fix). The quiet-NaN space above `0xFFF8` leaves seven 48-bit
   prefixes (`0xFFF9`…`0xFFFF`); kiln has nine pointer tags today (string, object, closure, buffer,
   symbol, bigint, temporal, cell, shared), so two must fold into another tag with a header type
   check (Temporal into object, shared into cell, for instance). Every `KILN_TAG_*` use and every
   generated fast path that tests a prefix is affected; the native corpus, GC stress and realms
   runs are the safety net. An ADR amendment to 0053.
2. **Keep kiln's memory below 2^47 on arm64 Linux**: link non-PIE so the program loads low, and
   give the collector's heap a low `mmap` hint. Smaller, but any pointer from elsewhere (the Rust
   provider's allocations, thread stacks) that reaches a `kiln_val` would still break, later and
   less obviously.

Until then the release workflow does not build `aarch64-unknown-linux-gnu`, and `install.sh`
says so on that platform.
