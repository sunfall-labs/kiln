# Private names are per class *definition*, not per class *evaluation*

**Status:** open. **52 test262 rows** -- 46 in `language/expressions/class`, 6 in
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
