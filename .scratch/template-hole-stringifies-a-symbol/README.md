# A template hole answers `String(x)` where the language says `ToString(x)`

**Status:** open, and **deliberately not fixed** -- 0 test262 rows, against a cost paid by every
template literal in every program. Recorded so the next person does not rediscover it.

## What is wrong

```js
const s = Symbol("x");
`${s}`     // node: TypeError.  kiln: "Symbol(x)"
"a" + s    // node: TypeError.  kiln: TypeError   <- already right
String(s)  // node: "Symbol(x)". kiln: "Symbol(x)" <- already right
```

`String(x)` and `ToString(x)` agree on every value in the language except one. A symbol is the
exception on purpose: `String(sym)` is the single sanctioned way to get a symbol's description,
and every other conversion to text refuses. A template hole is one of the ones that refuses.

## Why it is spelled this way

`crates/kiln-ir/src/compile.rs` compiles a hole as `String(expr)` -- `LoadBuiltin(String)`,
the expression, `Call{argc:1}` -- and the comment above it explains why that is not an accident:
a template reduces with the **string** hint, `toString` before `valueOf`, while `"" + x` reduces
with the default hint and tries them the other way round. Desugaring a template to `+` produced
the wrong answer for an object with a `valueOf` and no own `toString`. So `String()` is right for
the whole language except symbols, and `+` is wrong for a much more common case.

## Why it is not fixed

Three routes, all of which cost more than the bug does:

- **A `ToString` instruction.** The honest fix, and ADR 0043 makes it a change to both engines:
  a new instruction has to exist in the interpreter and the C backend or the table is a lie.
- **A new `Builtin` variant** loading a native that performs `ToString`. Smaller than an
  instruction, still both engines.
- **An emitted symbol guard**, the technique `guard_derived_return` uses -- around eight
  instructions per hole, with no new surface in either engine. Template literals are everywhere,
  including in hot string-building loops, so this is the one route that is *cheap to write* and
  expensive to run.

`language/expressions/template-literal` is **114 pass, 0 fail**, so none of this is worth rows.
It is worth knowing, because the failure is the quiet kind: a program that meant to catch a symbol
used as a key or a message gets a plausible string instead of the throw that would have named it.

If a `ToString` instruction is ever added for another reason, this is a caller waiting for it.
