# ADR 0023: The native microtask queue is per translation unit, until it cannot be

## Status

**Accepted and implemented.** Scalar units retain their private scheduler. Graph launchers use an
invocation-owned scheduler shared by all dependency-first module bodies, including executable and
shared-library paths.

## Context

`await` and `.then()` both mean "later, but before the program is finished". Something has to hold
what is waiting and run it after the synchronous part, and in the interpreter that something is
the VM: one queue, drained when the module body returns.

The native backend has no VM. Each module compiles to its own translation unit with its own entry
point, and the launcher calls those entry points in dependency order. So the question is where a
queue lives that every unit can reach, and who drains it.

Three answers were considered:

**1. A queue in the runtime, drained by the launcher.** `kiln-native-runtime` already declares
`kiln_value` for the embedding boundary, so it can hold the queue without interpreting it. But the
*runner* cannot live there: draining means calling a closure, and only generated code knows how.
The launcher would have to call back through a function pointer each unit registers on entry.
Correct, and it spans two crates.

**2. A queue in one designated unit.** There is no such unit: the entry module is not special to
the linker, and picking one would put a program's ordering at the mercy of which file it started
from.

**3. A queue per unit, drained when that unit's entry point returns.** What this implements.

## Decision

Each scalar generated translation unit owns a microtask queue, and its entry point drains that
queue before returning. Graph modules use a private seven-argument ABI carrying a scheduler handle.
The launcher/wrapper runs every synchronous module body first, then invokes one runner to drain
microtasks and virtual timers to quiescence. Closure calls switch each generated TU's thread-local
scheduler activation, so callbacks can cross module boundaries safely.

**For a single module this is exactly right**, and a single module is what `kiln run` compiles and
what every probe in `conformance/native` is. The body runs to completion, then the queue drains
until empty, which is the ordering node produces -- verified against it, not merely intended:
`conformance/native/promises.ts` compares the full interleaving of eleven queued reactions,
nested `async` calls included, between the interpreter, the native backend and node.

**For a module graph this now matches the intended boundary.** Node evaluates every module body and
then drains; the graph launcher follows that same order. The scheduler is invocation-scoped rather
than process-global, so concurrent and nested shared-library calls do not share queue or timer
state.

## Consequences

- The graph ordering guarantee is program-scoped. Cross-module promises, microtasks, and timers
  remain pending until all synchronous bodies have completed.
- The graph ABI is private and versioned independently from the public scalar/module ABI. Its
  cache identity includes the graph ABI and runtime versions, so runtime changes cannot reuse old
  graph objects or launchers.
- Deliberate constraint: the scheduler uses virtual time, as the scalar interpreter/native loop
  already does; real sleeping is outside this backend slice.
- Draining inside the entry point means an uncaught rejection surfaces as that module failing,
  rather than as a separate unhandled-rejection report. Node distinguishes the two; kiln does not,
  in either engine.
