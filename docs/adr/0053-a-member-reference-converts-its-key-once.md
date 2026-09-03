# ADR 0053: A member reference converts its key once, and plain assignment converts it last

## Status

Accepted, and implemented in both engines.

## Context

`o[k]` converts `k` with `ToPropertyKey`, and when `k` is an object that conversion is the
program's own `toString` or `valueOf` -- code that can count its own calls, and can throw.

Three forms read and write through *one* member reference: `o[k] += v`, `o[k]++`, and
`o[k] ??= v`. Each of them evaluated the key expression once, which is the part that is easy to
see, and then converted the resulting value twice: once inside `GetComputed` and once inside
`SetComputed`. A key object's `toString` therefore ran twice for a single `+=`, which the program
observes directly.

The specification does not describe this with an instruction. A Reference Record holds an
unconverted `[[ReferencedName]]`, and `GetValue` converts it and *stores the converted key back
into the record*, so the `PutValue` that follows finds a key already. The conversion is memoised
on the reference rather than repeated by each user of it.

Two details of the order matter and are easy to get backwards. Both were, here, before the tests
said otherwise:

- The conversion happens inside `GetValue`/`PutValue`, **not** where the member expression is
  evaluated. So `null[k] = v` throws for the nullish base before `k`'s `toString` runs, and
  `null[k] = expr()` evaluates `expr()` first and reports *its* error. Converting eagerly at the
  member expression broke 74 tests in `language/expressions` that assert exactly this.
- For a plain assignment the conversion is in `PutValue`, which runs *after* the right-hand side.
  For the three read-modify-write forms it is in `GetValue`, which runs *before*. The two are not
  the same order and cannot share one rule.

V8 implements the ordering but not the memoisation: `node` runs a key object's `toString` twice
for `o[k] += v`. Matching `node` was therefore not available as a way to decide this.

## Decision

A member reference converts its key once, where the specification converts it.

`ToPropertyKey` is one IR instruction. Its stack is object, then key; it replaces the key with
`ToPropertyKey(key)` and leaves the object alone. A nullish object passes through *unconverted*,
because the read or write that follows throws for it and that error comes first -- rather than
duplicating that error at a second site where it could drift.

It is emitted at the three read-modify-write forms and nowhere else. Compound assignment converts
on the stack before duplicating the pair; the update and short-circuiting forms already spill the
key to a hidden binding, so they convert and store the converted key back into that binding. All
three then read and write through the same key.

Plain assignment does not emit it. There the specification converts in `PutValue`, after the
right-hand side has run, and `SetComputed` still does that conversion itself.

Both engines also convert an object key inside the computed accessors themselves --
`computed_key` in the interpreter, `kiln_c99_reduce_key` in the prelude. That is what makes the
instruction's own conversion idempotent: by the time `GetComputed` sees a key the instruction
already reduced, the value is a primitive and no second call into the program happens.

## Consequences

The instruction is a permanent obligation in both engines (ADR 0043), bought for a spec corner
that `node` does not implement. It earns that by being uniform: one instruction, one meaning,
emitted wherever a member reference is used more than once. The alternative considered and
rejected was to apply it only to compound assignment, where the stack shape made it a three-line
change -- that would have left `+=` and `++` disagreeing with no principled account of why.

The native accessors gained the conversion as part of this. They did not have it: the prelude
reduced a key with `kiln_c99_property_key`, which answers `"[object Object]"` for every object, so
`o[{toString(){return "hello"}}] = 42` wrote under that name and `o.hello` read back `undefined`.
That is the silent wrong answer the backend is supposed to refuse rather than produce, and it was
invisible while both the read and the write agreed on the wrong name.
