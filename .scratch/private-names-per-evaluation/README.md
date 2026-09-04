# Private names are per class *definition*, not per class *evaluation*

**Status:** FIXED 2026-09-03, by the operand route described below. +53 rows measured
(`language/expressions` 20318 -> 20360, `language/statements/class` 8490 -> 8501). One gap is
left and is recorded at the end: `#x in o`.

**Status was:** open. **52 test262 rows** -- 46 in `language/expressions/class`, 6 in
`language/statements/class`, all named `*-brand-check-multiple-evaluations-of-class-*`.

## What is wrong

Every evaluation of a class expression creates fresh Private Names. Kiln keys a private member
by a string fixed at compile time, so two evaluations of one class share a brand:

```js
function factory() {
  return class {
    #m() { return 1; }
    static probe(o) { return o.#m(); }
  };
}
var A = factory(), B = factory();
A.probe(new A())   // 1, correct
A.probe(new B())   // TypeError in every other engine; kiln answers 1
```

`private_key` (`crates/kiln-ir/src/compile.rs`) builds `@@private:{scope}:{name}` from a
compile-time scope id. `{scope}` distinguishes one class *body* from another; nothing
distinguishes one *evaluation* from the next.

The brand check itself works -- `#m` on a foreign object, a static private member reached through
an instance, an assignment to a private method are all refused correctly, and were verified
against node. The brand is simply too coarse.

The `-eval` and `-eval-indirect` variants pin the same point through `eval`, and `-factory` and
`-function-ctor` through a closure and the `Function` constructor: it is the evaluation that
mints the name, not the source text.

## Why it is not small

The key has to gain a runtime component, which makes every private access dynamic:

- the class evaluation must mint a fresh identity and keep it somewhere the member accesses can
  reach -- a captured binding would do, and is the shape already used for the private methods and
  the computed field names on this branch;
- but each access site currently emits a *constant* key, so it would have to build the key from
  that binding instead, turning `this.#m` from a named read into a computed one. That is a
  codegen and performance change, not a table edit.
- and the C backend keys its private members the same way, so both engines change together
  (ADR 0043).

An alternative worth weighing first: keep the static key for the *slot* and give each evaluation a
brand object that instances carry, checked before the slot is read. That leaves accesses named and
puts the per-evaluation identity in the check rather than in the key -- likely the smaller change,
and closer to how the specification describes it.

## What the surface actually is (measured, not assumed)

Private access is **not** a generic property read: it has three instructions of its own --
`GetPrivate(u32)`, `SetPrivate(u32)` and `DefinePrivate { .. }` -- emitted from 11 sites in
`compile.rs`. `GetPrivate`'s own comment states the current rule plainly: "Own-only, and a miss is
a TypeError. That own-only rule *is* the brand check." So the check exists and is in one place;
only its granularity is wrong.

That makes the brand-object design concrete, and there are two ways to carry the brand to the
access site:

1. **A stack operand.** The three instructions take the brand as an extra operand, pushed by
   `load_captured_binding` from a hidden binding the class evaluation fills. One instruction
   shape change, three interpreter arms, three C emissions. The check stays a single comparison
   in one place per instruction.
2. **An emitted guard.** No instruction changes at all: before each access, compare the
   instance's stored brand against the class's, and throw if they differ, using only instructions
   that already exist. This is the shape the iterator-result, derived-constructor-return and
   static-`prototype` guards on this branch already use, and it reaches both engines for free --
   but it puts ~8 instructions in front of every private access, where option 1 puts one
   comparison inside the instruction that is already running.

Option 1 is the better trade for a path this hot; option 2 is the one to reach for if the
cross-engine cost of changing three instruction shapes turns out to be the obstacle. Either way
the brand is minted once per class evaluation and installed on the instance beside the private
members, which is where `install_private_and_fields` already runs.


## How it was done

The operand route, as recommended. Each class *evaluation* mints a brand -- an empty array, the
cheapest thing with an identity -- into a hidden binding declared where the class opens its
private scope, so every method compiled inside captures it. The brand is installed on each
instance beside its private members, and on the constructor, under `@@private:{scope}:%brand`:
one per class, not one per member, because the question is which evaluation an object came from.

`GetPrivate` and `SetPrivate` take the brand as an operand pushed directly beneath them, so each
of the nine access sites needed only a local edit and no reordering. Both engines compare the
instance's stored brand against it and raise the same TypeError the own-only rule already raised.
The C backend derives the brand key from the private name at emission time, so the instruction
keeps the shape the interpreter gives it.

An absent brand -- a private access compiled outside any class, which is a syntax error reported
elsewhere -- is let through, so the error that matters is the one already raised.

## What is still missing

`#x in o` compiles to the ordinary `in` against the private key, so it still answers `true` for an
instance of a *different* evaluation where it should answer `false`. It needs the brand comparison
too, and unlike the accesses it must answer `false` rather than throw. The emitted form would be
`(key in o) && o[brandKey] === brand`, which existing instructions can express.
