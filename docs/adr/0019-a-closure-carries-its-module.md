# ADR 0019: A closure carries its module

## Status

Accepted.

## Context

Each module compiles to its own `Program` and executes in its own VM. A closure was a
`(function_index, upvalues)` pair, and the function index was resolved against whichever program
the *running* VM held. Module-scope bindings were resolved the same way: a function reading one
compiled to `LoadGlobal(slot)`, which reads the module body's frame at the bottom of the stack.

Both work exactly as long as a function value never leaves the module that made it. Neither
survives it leaving.

`export function greet() { … }` is the ordinary way to publish a function, and it did not work at
all: calling the imported value indexed the *importer's* function table and found either someone
else's body or nothing, reported as "no compiled function at index 0". The `LoadGlobal` half was
worse, because it did not fail. A function exported from a module that read one of its own
module-scope constants got whatever the importer happened to have in the same-numbered slot —
a wrong answer, silently, and one that changes depending on how many bindings the importer
declared before it.

This was not visible because nothing exercised it: the Effect corpus is bundled to one file per
program precisely because kiln had no module story, and the module examples exported only
numbers.

## Decision

**A closure holds an `Rc<Program>`.** `function` indexes that table, not the VM's. The machine's
`program` field becomes the *running frame's* program, saved into the frame on call and restored
on return, exactly like `this` and the locals. `code()` reads from a local `Rc` clone rather than
from the machine, which is what lets the interpreter loop hold an instruction across the `&mut
self` calls that execute it.

**Module scope is an ordinary capturable scope.** The exemption that kept module bindings out of
cells is gone: a module-scope binding a nested function refers to is boxed like any other, read
through an upvalue, and travels with the closure. `capture()` no longer skips module scope, and
the import prologue moves each captured import from its raw slot into a cell before the body
runs — the host installs import values directly in slots, so the boxing cannot happen at a
declaration the way it does for a `let`.

`LoadGlobal` stays for the bindings no nested function refers to, where the frame lookup is
correct and cheaper.

## Consequences

`export function`, `export class`, and `export default function`/`class` now lower and run, which
is what prompted this. An exported closure keeps its own module's constants, function table, and
captured bindings, so a module is a real unit rather than a fragment that only works in place.

The costs are real and bounded. A module-scope binding read by a function is one pointer chase
instead of a frame index; in a bundle, that is most reads, and the recompute counters the
performance harness gates on are unchanged. `execute` now takes an `Rc<Program>`, which its
callers already own. `execute_module` clones the module's program into one per execution, because
a function value it exports outlives the run that made it — holding the program by reference
would tie every exported function to one module execution.

Export extraction reads through a cell when the exported slot holds one. That takes the value at
the end of the module body, which is what the previous implementation did; genuinely live
bindings — where the importer sees a later reassignment — remain unimplemented, and would follow
from exporting the cell rather than its contents.
