# ADR 0023: The native microtask queue is per translation unit, until it cannot be

## Status

**Accepted for a single module; a known approximation for a graph.** The correct design for the
graph case is written down below and is a small change to `kiln-native-runtime`, which is why it
is recorded rather than deferred silently.

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

Each generated translation unit owns a microtask queue, and its entry point drains that queue
before returning.

**For a single module this is exactly right**, and a single module is what `kiln run` compiles and
what every probe in `conformance/native` is. The body runs to completion, then the queue drains
until empty, which is the ordering node produces -- verified against it, not merely intended:
`conformance/native/promises.ts` compares the full interleaving of eleven queued reactions,
nested `async` calls included, between the interpreter, the native backend and node.

**For a module graph it is an approximation.** Node evaluates every module body and *then* drains;
this drains after each one. A promise chain that spans modules can therefore interleave
differently. Nothing in the corpus does that today, and the difference is visible rather than
silent -- `scripts/native.sh` compares the two engines on whatever is put in front of it.

## Consequences

- The ordering guarantee is stated at module scope rather than program scope. Any program that is
  one module -- the common case, and every probe here -- gets node's ordering exactly.
- A cross-module `await` may run earlier than it would in node. That is a real difference and it
  is not detected automatically; it needs a graph probe, which does not exist yet because the
  native module path has no async coverage.
- The fix is design 1 and it is small: a queue and a `int (*runner)(kiln_host *)` in
  `runtime_source()`, each unit assigning the runner on entry, and the launcher calling it after
  the last module. It is not done here because it lands in a crate another session owns and
  because the single-module case -- which is what the compiler is used for today -- is already
  correct.
- Draining inside the entry point means an uncaught rejection surfaces as that module failing,
  rather than as a separate unhandled-rejection report. Node distinguishes the two; kiln does not,
  in either engine.
