# ADR 0030: `eval` takes an injected compiler, and a direct one takes a compiled scope

## Status

Accepted and implemented for the interpreter. The native backend refuses a direct `eval` and
does not provide the indirect one.

## Context

Around 1,500 Test262 variants failed with `ReferenceError: eval is not defined`, and another 150
on `new Function(source)`, which shares the machinery. Both need a compiler while a program is
already running — the one thing an ahead-of-time pipeline is arranged not to have.

Two problems, and they are unrelated to each other.

**Where the compiler comes from.** ADR 0007 puts `kiln-interpreter` downstream of the bytecode
and clear of the frontend: it depends on `kiln-ir` and `kiln-host-abi`, and on neither HIR nor
Oxc. An `eval` that parsed its own source would pull Oxc's dependency graph into every build of
the VM and make a parser upgrade a VM rebuild — which is the coupling ADR 0007 exists to prevent.

**What a direct eval can see.** `eval("x")` called by that name evaluates in its *caller's* scope.
The caller's scope is gone by run time: the compiler resolved every name to a slot number, and a
slot number means nothing to a program compiled separately. Worse, the two failure modes are not
symmetric. A name the eval cannot find throws a `ReferenceError`, which is loud. A name it finds
in the *wrong* scope answers with a plausible value — `function f() { var Array = 7; return
eval("Array"); }` would hand back the constructor — and nothing downstream can tell.

## Decision

**The frontend is injected, exactly as it is for the query engine.** `kiln-interpreter` declares a
`SourceCompiler` function pointer over owned data and a thread-local slot to install one in. This
is the shape ADR 0006 already gives `kiln-db`, which takes `frontend_oxc::parse` as a `Frontend`
function pointer rather than depending on the parser. `kiln-conformance` installs one built from
`frontend_oxc::parse_with_goal` + `kiln_ir::compile_eval`; a host that installs none has no
`eval`, and says so rather than answering.

**A direct eval is an instruction, not a call.** `Instruction::DirectEval` carries an `EvalSite`:
the names visible where the call is written, how the machine reaches each one, and whether the
caller is strict. A value could not carry any of that, which is why every *other* route to the
same function — `(0, eval)(s)`, `globalThis.eval(s)`, a saved reference — is an ordinary call of
`Builtin::Eval` and is an indirect eval, evaluating in global scope.

**Bindings are handed over as cells, so a function containing an eval boxes all of them.** The
eval'd program is compiled against slots of its own, and a shared cell is the only storage two
independently compiled programs have in common. It has to be a cell rather than a copy, because
`eval("x = 2")` must be visible to the caller afterwards. So `Compiler::cell_everything` boxes
every binding of any function containing a direct eval, and of every function around it. That
costs one pointer chase per access in a function that already contains an eval, and buys the
guarantee that *every* name at the call site can be handed over — the alternative is dropping the
ones that cannot, which is the silent-wrong-scope failure above.

**An eval program is a second module scope.** It has top-level slots of its own, so it enters a
frame like a call does and `Vm::global_base` moves with it: `LoadGlobal` from inside an eval finds
the eval's own frame rather than the caller's module frame, where the same slot numbers hold
unrelated bindings. `compile_eval` boxes its top-level bindings unconditionally so that no nested
function inside an eval reaches for module scope at all, and the machine refuses a compiled eval
program that contains `LoadGlobal` anyway — it has never fired, and a wrong binding is worse than
a refusal.

**Strictness comes from the caller, because the text no longer says.** Oxc keeps a directive
prologue beside the body rather than in it, and the frontend lowers only the body — so `"use
strict"` does not survive into HIR and the compiler cannot see it. `CompileOptions::strict` is how
a caller says; the interpreter also scans the eval'd source's own prologue textually, since that
is the other way an eval becomes strict.

## Consequences

`eval` in all three forms works in the interpreter: direct, indirect, and `Function(source)`.
Measured against the baseline, with no test made to pass by weakening an assertion:
`language/eval-code` 16 → 248 passing variants, `built-ins/eval` 0 → 16, `built-ins/Function`
265 → 340, `language/expressions/call` 131 → 147, and `language/statements` 12,529 → 13,687 as a
control. Peak RSS is unchanged (`built-ins/Array`, the heaviest area, stays at 1.1 GB).

**A sloppy direct eval's `var` does not reach the caller unless the caller already has the name.**
`eval("var x = 1")` where the caller declares `x` writes the caller's binding, which is correct.
Where the caller does not, the binding stays in the eval's own frame and a later read from the
caller throws `ReferenceError` rather than answering 1. Closing that needs a variable environment
the caller can grow at run time, which every unresolved-name read would then have to consult — a
larger change than this one, and the failure is loud in the meantime.

**A completion value comes only from a top-level expression statement.** `eval("if (true) 1")` is
`undefined` where the specification says 1, and `eval("'ok'")` is `undefined` because a leading
string literal is a directive and the frontend drops it. Both are gaps rather than wrong answers:
nothing else can claim to be the result.

**The native backend refuses a direct `eval`.** It ships without a compiler, and `LoadBuiltin` of
`Builtin::Eval` is refused for the same reason. Neither is approximated, so the differential
harness sees a refusal rather than a divergence.

**`kiln run` has no `eval` yet.** The compiler is installed by the conformance runner; the daemon's
execution path needs the same one line beside its existing `parse_with_goal`/`compile` pair.
