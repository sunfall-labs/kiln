# An array's `length` cannot exceed what the dense storage materialises

**Status:** open, and deliberately not attempted.

**Corrected 2026-09-03, shortly after filing.** This was first written up as "133 rows, one
root cause". That was wrong: two different faults shared the `Invalid array length` message.
About 26 of those rows were a *coercion* bug -- `array_set_length` read its value with the pure
`as_number`, so `a.length = {toString(){return "2"}}` answered NaN and was refused as an invalid
length. That one is fixed and the rows are recovered; it had nothing to do with the dense
representation. What is left below is the genuine limit, and it is smaller than first claimed.
Do not trust a row count that was derived from a shared error message without checking that the
tests behind it fail for the same reason -- that is exactly how this one went wrong.

## What happens

`ArrayData` keeps its elements in `items: RefCell<Vec<Value>>` and derives `length` from
`items.borrow().len()`. A length beyond `MAX_DENSE_ARRAY` therefore cannot be represented at
all, and `array_set_length` refuses it:

```
array of length 4294967294 exceeds this implementation's dense representation
```

So `new Array(2**32 - 2)`, `a.length = 2**32 - 2`, and
`Object.defineProperty(a, "length", {value: 2**32 - 1})` all fail, as does any test that reaches
a very high index.

## Where the rows are

| rows | chunk |
| --- | --- |
| 54 | `built-ins/Object` (mostly `defineProperty`, `defineProperties`) |
| 50 | `built-ins/Array` |
| 11 | `staging` |
| 7 | `built-ins/Iterator` |
| 5 | `built-ins/TypedArray` |
| 3 | `intl402` |

98 rows say `dense representation`; a further 35 say `RangeError: Invalid array length`, which
is the same limit reached by the other door.

## What a fix costs

`length` has to become a field of its own, independent of what `items` holds, and every dense
fast path has to stop assuming the two agree -- around 30 direct uses of `items.borrow().len()`
plus the paths that iterate elements directly. Arrays are the hottest structure in the engine,
and the C backend has its own array representation that would need the same treatment for the
two to keep agreeing (ADR 0043).

Two things already in place would help and are worth knowing about: `elements:
RefCell<Vec<ExoticElement>>` already holds index properties the dense storage cannot express,
and the `sparse` flag already forces an array with holes off the fast paths. A sparse `length`
would extend that machinery rather than invent it.

## Why it is filed rather than done

No program makes a four-billion-element array. The rows are real but the behaviour behind them
is not something real code reaches, which is the opposite trade from the other work on this
branch -- the regexp `lastIndex` change was 56 rows *and* a correctness fix a program could hit,
and the class-element changes were 74 rows *and* fixed identity and evaluation counts that
ordinary code observes.

This one is the largest row count and the largest, riskiest change, in the hottest data
structure, for a case nothing real hits. If the aim is the headline percentage rather than the
engine, it is the single biggest lever left: 133 rows, about 2% of the remaining failures.
