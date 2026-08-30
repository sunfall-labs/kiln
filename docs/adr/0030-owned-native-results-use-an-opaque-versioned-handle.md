# ADR 0030: Owned native results use an opaque versioned handle

## Status

Accepted.

This extends ADR 0009's native embedding boundary without changing its ABI-v1 entry point. It is
compatible with ADR 0028's explicit runtime/resource ownership and with ADR 0021's requirement that
the eventual collected value representation remain private.

## Context

The ABI-v1 entry point writes a scalar `kiln_value`. Its string pointer is borrowed: a HostOps
callback only has to keep a returned string alive until the current generated entry returns, and a
generated string may be static or backed by an allocation with no public owner. The graph wrapper
copies callback strings soon enough for later modules to consume them, but releases that invocation
arena before returning. ABI v1 therefore cannot promise that an arbitrary public string result is
retained after the call.

The ownership rule cannot be inferred from `kiln_value.ptr`. The pointer may name static generated
bytes, callback-owned bytes, current process-lifetime C storage, or a future realm heap. Adding a
`free` bit to the scalar would make one layout carry different ownership rules in HostOps arguments,
HostOps results, graph imports and public results. An inline header disposer would also risk freeing
with a different C allocator on platforms whose host and library use different runtimes.

A caller-buffer sizing call is not general: rerunning a program to fill a larger buffer repeats its
HostOps effects. Retaining the completed invocation merely moves the ownership problem into an
implicit global table.

## Decision

### ABI v1 remains unchanged

Each embedding library continues to export its existing content-derived
`int entry(kiln_host *, kiln_value *)` symbol. Existing hosts and the borrowed-result contract keep
working byte for byte. `kiln_value` and the HostOps ABI remain version 1.

The library additionally exports one deterministic `entry_api_v2` accessor. The generated header
declares an opaque `kiln_result_v2` and this descriptor:

```c
typedef struct kiln_result_v2 kiln_result_v2;

typedef struct kiln_module_api_v2 {
    uint32_t abi_version;
    uint32_t struct_size;
    int (*run)(kiln_host *host, kiln_result_v2 **result);
    int (*read)(const kiln_result_v2 *result, kiln_value *value);
    void (*dispose)(kiln_result_v2 **result);
} kiln_module_api_v2;
```

The accessor returns an immutable process-lifetime descriptor whose version is 2 and whose size
lets a host reject incompatible extensions. The implementation functions remain private dynamic
symbols; exposing them through the library-specific descriptor prevents two loaded Kiln libraries
from interposing a common allocation/disposal symbol.

### One handle owns one normalized scalar snapshot

`run` first stores `NULL` in the caller's output. Once evaluation produces a boundary scalar, it
allocates one opaque handle and copies that scalar into it. A string is copied byte for byte,
including embedded NUL bytes and the empty string, into storage owned by the handle. Other scalar
tags require no payload allocation. No pointer into a HostOps provider, generated temporary, or
realm heap crosses this interface.

`read` copies a borrowed `kiln_value` view out of the immutable handle. Its string bytes remain
valid until that handle is disposed. The caller may read a handle repeatedly, but must not copy its
ownership or race disposal with a read.

`dispose` accepts a pointer to the caller's handle variable, releases storage with the same library
that allocated it, and stores `NULL`. Passing a null pointer or an already-null handle is a no-op,
so cleanup paths are idempotent. Every handle must be disposed before unloading its library because
the descriptor's function pointers and allocator owner cease to exist at unload.

### Failure values follow the same ownership rule

If evaluation returns a JavaScript status such as `KILN_C99_THROWN`, `run` preserves that status
and still returns a readable handle when the boundary value was captured successfully. An invalid
callback string produces `KILN_C99_INVALID_ARGUMENT`; failure to allocate the snapshot produces
`KILN_C99_HOST_ERROR`, and either leaves the output null. The public header declares every status
the backend can return.

Scalar v2 calls interpose the same invocation-local HostOps proxy used by graph embedding. It copies
a callback string before returning control to generated code, while the callback contract still
guarantees the source bytes. Graph v2 wraps the existing graph entry and copies the final result
before the outer proxy arena is released. This is intentionally per invocation: repeated,
reentrant and concurrent calls share no result owner.

### The new surface participates in artifact identity

The module-API version, generated wrapper bytes, ordered public export names and actual final-link
toolchain participate in generated-source, object, loader and linked-artifact identity. A build
report and daemon response name both the legacy entry symbol and the v2 descriptor accessor. The
program-specific header is the authoritative declaration for both.

## Consequences

- C and C++ embedders can retain scalar string results for an explicit lifetime without guessing
  their origin or allocator.
- ABI-v1 callers are not silently given a new disposal obligation.
- The owned handle is a boundary snapshot, not Kiln's internal value representation. A future
  moving collector or Tokio-owned realm can change without invalidating this scalar contract.
- Current generated concatenation and conversion allocations may still be retained for process
  lifetime. This decision fixes public result ownership; it does not claim that the interim C heap
  is leak-free.
- Objects, functions and other future reference values require a separately designed opaque realm
  handle or serialization contract. They are not smuggled through this scalar handle.
- One small allocation is required per v2 result, trading throughput for an unambiguous ownership
  boundary. ABI v1 remains available where the caller can consume a borrowed result immediately.

## Alternatives rejected

**Put an ownership flag in `kiln_value.reserved`.** This is smaller, but makes the same scalar
contextually owned or borrowed and encourages disposal with the wrong allocator.

**Return caller-sized bytes directly.** The required size is only known after a potentially
effectful execution. Retrying can duplicate I/O and retaining a hidden completed call is an opaque
handle with worse lifecycle control.

**Accept host allocator callbacks.** That couples every generated/runtime allocation helper to an
invocation allocator even though only the boundary snapshot needs host-visible ownership.

**Expose a raw internal or realm pointer.** It would freeze the pending value/collector decision,
require pinning across safepoints, and let a public ABI outlive the realm that owns the value.
