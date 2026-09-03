# 02: A collector hook for the turn-boundary dump

Status: done

## What

The drive loop is Rust; the dump is C. Rust needs a way to enter the prelude at the turn boundary.

## How

`kiln_gc_hooks` (ADR 0041) is already exactly that channel: the archive holds an opaque heap
pointer whose first word is the prelude's hooks table, and calls through it at the safepoint.
Add one entry.

- `crates/kiln-backend-c/src/runtime/prelude_types.h`: `int (*introspect)(void *heap);` last in
  `kiln_gc_hooks`.
- `crates/kiln-backend-c/src/runtime/prelude_runtime.c`: `KILN_GC_HOOKS_VERSION` 1 → 2, and the
  new function in the table initialiser.
- `crates/kiln-native-number-ffi/src/lib.rs`: the field on `GcHooks`, `GC_HOOKS_VERSION` 1 → 2, the
  `kiln_realm_gc_attach` validation, and `introspect_if_requested(realm)` called from
  `drive_async` immediately after `collect_if_due(realm)`.

## Return value

A bitmask, because the caller needs two different facts from one call:

- `1` armed — this process installed a `SIGUSR1` handler, so the drive should bound its park
  (issue 04).
- `2` dumped — a request was pending and has been served.

## Why not a new root phase

`kiln_realm_gc_register_roots` takes a phase and would need no ABI change. It is the wrong shape:
every translation unit registers a root function, so a phase would run the check once per unit and
a process with N units would print N dumps. The hooks table is reached through the heap and there
is exactly one heap, so it enters exactly one unit's copy of the prelude — the same unit that armed
the handler.

## Done when

`emitted_c__runtime_prelude_abi_surface` is re-accepted with the widened table, and a realm still
attaches (an unchanged `scripts/native.sh`).
