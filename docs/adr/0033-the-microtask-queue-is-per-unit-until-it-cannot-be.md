# ADR 0033: The native microtask queue is per translation unit, until it cannot be

## Status

**Superseded by ADR 0028.** This document records the former per-translation-unit approximation.
The Realm-owned correction described below is now implemented for Promise reactions,
`queueMicrotask`, `process.nextTick`, timers and native HTTP event ingress. The native next-tick
lane has its own Realm queue and drains before ordinary microtasks at every checkpoint.

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

**3. A queue per unit, drained when that unit's entry point returns.** What the original
implementation chose.

Implementation update (31 August 2026): generated C now submits opaque microtask payloads through
the private Realm ABI, the native launcher gives every module in a graph the same Realm, and it
drives that Realm only after the complete graph has entered. The per-unit queue described by this
ADR is no longer the implementation. A distinct private-ABI `process.nextTick` lane was added in
commit `b0869ee8`; its FIFO drains before ordinary microtasks at every
checkpoint, including when a nextTick callback queues another nextTick. Cross-module coverage in
`kiln-module-graph` and the public native probe both pin that ordering.

## Decision (superseded)

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

- Microtask ordering is now a Realm/program invariant rather than a translation-unit invariant.
- A graph shares one queue and reaches its first checkpoint after dependency-first module entry,
  removing the cross-module approximation this ADR accepted temporarily.
- Native `process.nextTick` used a private async ABI revision and cache-identity miss without
  changing the public embedding ABI or an IR discriminant.
- Unhandled-rejection reporting remains a separate compatibility question; moving queue ownership
  did not silently claim Node's complete reporting policy.
