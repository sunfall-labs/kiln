# 03: The signal handler, the flag and the formatter

Status: done

## What

One new self-contained section in `prelude_runtime.c`, placed between `kiln_c99_gc_trace_count`
and `kiln_c99_gc_heap`, plus one line in `kiln_c99_gc_heap` and one in the hooks table. Nothing
existing is reformatted: another workstream is editing `prelude_runtime.c` and `prelude_core.c` at
the same time.

## Rules the handler obeys

- Assigns `1` to a `volatile sig_atomic_t` and returns. No `malloc`, no `printf`, no lock, no read
  of the heap.
- The heap read has to be deferred whatever else is true, because the kernel may run the handler
  on any thread that has not blocked the signal, including one of Tokio's blocking-pool threads.
- `sigaction` with `SA_RESTART`, so an interrupted `accept`/`epoll_wait` resumes rather than
  failing the server with `EINTR`.

## Rules the formatter obeys

Written to the async-signal-safe rules even though it runs at the turn boundary: a stack buffer,
hand-rolled base-ten spelling, one `write(2)` per line to fd 2, retry-bounded on a short write, no
`errno` read. `stdio` is out for the locale reason already recorded beside
`kiln_number_to_js_string`, and one `write` per line is what makes a dump racing another writer on
fd 2 interleave lines rather than characters.

Note in the code that only the `heap` line is actually handler-reachable — `runtime` and
`connections` call into the archive — so the discipline is a property worth keeping rather than a
capability being used.

## Arming

`KILN_INTROSPECT=0` opts out, spelled like `KILN_GC=0`; armed otherwise. Arm exactly once, in the
translation unit whose candidate heap won `kiln_realm_gc_attach`, so the static flag the handler
sets is the one the hook (reached through `heap->hooks`) reads. The prelude is emitted per unit
with internal linkage, so any other placement would arm one unit's flag and poll another's.

Guard the POSIX parts with `#if defined(__unix__) || defined(__APPLE__)`; a non-POSIX target gets
a hook that reports "not armed" and no signal handling. Add `<signal.h>` and `<unistd.h>` to
`HEADER` under the same guard.

## Ownership guard

`kiln_c99_introspect_pending` and `kiln_c99_introspect_armed` are new mutable `kiln_c99_*` statics
and must be classified in `C99_MUTABLE_STATE_OWNERS`. Neither is realm state, frame state, a test
fixture, or derived from constants: they belong to the process and its signal disposition, which
outlive any one realm. Add a `Process` owner.

## Done when

`every_mutable_c99_static_has_an_explicit_owner` passes, and a compiled server writes three lines
on `SIGUSR1` and keeps serving.
