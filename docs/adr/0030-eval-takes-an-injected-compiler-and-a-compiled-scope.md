# ADR 0030: `eval` takes an injected compiler, and a direct one takes a compiled scope

## Status

Accepted and implemented for the interpreter. The native backend refuses dynamic source until
ADR 0036's realm-owned value kernel and injected native compiler service are implemented; it does
not approximate direct eval, indirect eval, or `Function` through a second heap.

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

**The frontend is an explicit realm execution capability.** `kiln-interpreter` declares a
`SourceCompiler` function pointer over owned data and carries it in `ExecutionCapabilities` from
each public execution boundary into the VM. There is no process-global or thread-local installer:
two realms on one Tokio thread may select different compiler authority, and an execution cannot
inherit one from whatever ran before it. This keeps the shape ADR 0006 gives `kiln-db`—the VM
still depends on no parser—while satisfying ADR 0031's rule that realm authority is named.
`kiln-conformance` supplies a value built from `kiln_frontend_oxc::parse_with_goal` +
`kiln_ir::compile_eval`; a caller that supplies the default capabilities has no `eval`, and says
so rather than answering.

`ModuleLoader` authority travels in the same capability value. The former thread-local loader and
thread-local referrer base were removed rather than retained as a compatibility path. A future
filesystem loader must be a scoped, referrer-aware service; no execution may inherit resolution
authority from an earlier run on the same worker thread.

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

**The native backend refuses dynamic source.** It ships without a compiler service, and
`LoadBuiltin` of `Builtin::Eval` is refused for the same reason. `Function` is present as a global
constructor so feature detection remains structurally honest, but invoking it cannot produce a
callable until ADR 0036's shared value-kernel handle exists. None of the three forms is interpreted
in a second heap or compiled ambiently, so the differential harness sees a refusal rather than a
plausible value with broken identity.

**Executable interpreter paths select Oxc explicitly.** The daemon and conformance runner pass the
same capability value through script and module-graph execution, including lazy CommonJS modules.
Compiler-free embedders and benchmarks pass the default value explicitly.
