# ADR 0065: Engines implement a declared primitive set; built-ins are TypeScript over it

## Status

Accepted, 2026-09-26. Decided by the project owner.

Originally proposed, 2026-09-26.

Makes concrete ADR 0005's rule that built-ins are "written against HostOps and nothing else", and
supplies the mechanism behind ADR 0060's bootstrap-TypeScript owner kind. Extends ADR 0054 and
ADR 0059 (a built-in member is described once) to what the member's implementation is allowed to
call.

## Context

ADR 0005 decided that built-ins are TypeScript compiled by the ordinary query pipeline, and that
their only seam to the engine is a versioned HostOps ABI. The first half happened: `kiln-builtins`
holds Date, `%TypedArray%`, the Error subclasses, Annex B, DisposableStack, ArrayBuffer, the web
platform classes and the `node:*` modules, and `common_bootstrap_source()` feeds the same source to
the interpreter and the native backend.

The second half did not. The bootstrap TypeScript reaches the engines through two channels, and
neither is declared anywhere.

**It captures public members at bootstrap.** `date.ts` begins by reading `Date.prototype.getTime`
and `setTime` and builds the whole prototype on them. That works, but it means the engine
primitive is whatever public member the TypeScript happened to capture, so an engine cannot tell
which of its members are load-bearing for the bootstrap and which are ordinary built-ins waiting
to be migrated. Every member is both.

**It reads `process.__kilnNativeModules`.** `node/fs.ts` and seven other modules look up host
functions by string on a property both engines install on `process`
(`kiln-interpreter/src/lib.rs`, `kiln-native/src/lib.rs`). The channel is observable to user code,
untyped, and falls back silently: a missing entry becomes a function that throws at the call site,
naming the module but not the missing binding.

The consequence is the one ADR 0060's revision records. Because nothing says which engine members
are primitives, nothing says which ones may be deleted when a family moves to TypeScript, so none
are. The engines keep about 600 `Native` variants and 470 C method codes, most of them members
with a full observable algorithm, and every one of them is a second or third implementation.

The IR already has the right shape for a declared seam. `Instruction::HostCall { op, argc }`
carries a numeric `HostOp`, and both engines dispatch on it. It is used today only for the handful
of embedder services in `kiln-host-abi` (stdout, clock, entropy, timezone, locale).

## Decision

**Each engine implements a small, declared set of primitives. Every built-in member above them is
TypeScript in `kiln-builtins` and has no engine implementation.**

### One declaration

Primitives are declared once, as a typed table in `kiln-builtin-surface` beside the member surface
of ADR 0043. Each row gives:

- a stable numeric id and a name;
- its parameter and result types, in the vocabulary the value graph already uses (tagged value,
  number, UTF-16 string, object of a named internal kind, host handle);
- whether it can throw, and which error kinds, if so;
- which class it belongs to (below);
- its reason: why it cannot be TypeScript over the other primitives.

From that table the build generates, and a test verifies against the checked-in output:

- the Rust trait the interpreter implements, so a missing primitive is a compile error;
- the C prototypes the prelude implements, so a missing definition is a link error;
- a `.d.ts` module that `kiln-builtins` imports, so a wrong call is a type error in the TypeScript.

### One channel

Bootstrap TypeScript calls a primitive by importing it from the reserved specifier
`kiln:primitives`. The compiler lowers each call to one `Instruction::Primitive { id, argc }`, which
the interpreter dispatches through the generated trait and the native backend emits as a direct
call to the generated C function. The specifier resolves only for sources compiled as built-ins;
user code that imports it is refused at resolution with the reason.

`HostCall` stays for embedder services, and a primitive whose class is *host* is a `HostOp` under
this same declaration. The two instructions differ in who implements the operation, not in how it
is declared.

`process.__kilnNativeModules` and capture-at-bootstrap of public members are both retired. Each
existing use migrates to a declared primitive; the property is removed from both engines when the
last use is gone.

### What may be a primitive

A primitive earns its row for one of three reasons, and the row names which:

1. **Representation.** It reads or writes storage whose layout is engine-specific: an internal
   slot, a string's code units, an array's dense elements, a shape check, a weak reference, a
   generator's frame. These are ADR 0060's representation operations.
2. **Host.** It needs authority the realm does not have: I/O, the clock, entropy, the environment.
   These are ADR 0005's HostOps.
3. **Measured.** The TypeScript over the other primitives misses a recorded budget on a recorded
   workload. The row carries the workload and both numbers. A measured primitive is also a
   temporary duplicate under ADR 0060 and keeps the TypeScript as its fallback.

A primitive performs no specification coercion and selects no error that a caller could select.
It receives values the TypeScript has already validated and converted, and does one thing to
engine storage or the host. `ToString`, `ToIntegerOrInfinity`, argument defaulting, `RangeError`
versus `TypeError`: all of that is written once, above the line.

### The set is meant to stay small

The primitive count is reported beside ADR 0060's duplicate count. It is not capped, because a cap
would push work into TypeScript that should be a primitive, but a new row is a reviewed change to
one table rather than a side effect of implementing a member.

## Consequences

- Moving a family to TypeScript has a definite end: the TypeScript calls only declared primitives,
  and every engine implementation of the family's members is deleted (ADR 0060).
- The engines shrink to representation, host and measured primitives, plus the value graph and
  interpreter machinery. That is the code that genuinely must differ between them.
- The interpreter pays a call per primitive where it used to run a whole member in Rust. For
  string and array families that may be slower in the interpreter; the interpreter's job is the
  edit-run loop and differential reference, not throughput, and a measured primitive is the answer
  where it matters.
- The native backend sees primitives as direct calls with declared types, which the value graph can
  inline or specialise. Today it sees a captured public member, which is an ordinary property read
  and call.
- `kiln-builtin-surface` becomes the single place to look for what a member is and what it may call,
  which is what a `kiln explain <member>` command reads.
- The TypeScript built-ins get compile-time checking against the real engine seam instead of
  `as any` over a dictionary.

## Rejected alternatives

**Keep capturing public members.** It needs no new mechanism, but it makes every public member a
potential primitive, so no member can be deleted with confidence. It also makes the bootstrap
depend on members that user code can observe and, before bootstrap completes, that other bootstrap
code could have replaced.

**Give primitives a global object, like `__kilnNativeModules` but typed.** Typing the dictionary
fixes the silent fallback but keeps the primitives observable to user code and keeps every call a
property read that the native backend cannot resolve statically.

**Implement primitives as `HostCall` with more `HostOp` values.** `HostOp` is the embedder ABI
(ADR 0005, `kiln-host-abi`), versioned for embedders. Most primitives are engine-internal and change
with the engine; putting them in the embedder ABI would version them at the wrong boundary.

**Write the built-ins in Rust once and call them from C through the provider.** This is the right
answer for algorithms that need Rust (ADR 0060 kinds 1 and 2). For ordinary members it makes every
member a provider crossing, which ADR 0060 already rejects as the wrong granularity, and it moves
built-in edits back into the Rust rebuild loop that ADR 0005 exists to leave.
