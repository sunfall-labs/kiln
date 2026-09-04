# `String.prototype.normalize` does not normalise

**Status:** open, and deliberately left open. The *form* argument is now validated; the
normalisation itself is still a no-op.

```js
"é".normalize("NFC").length   // 1 in every other engine; 2 in kiln
```

`Native::StringNormalize` answers the receiver unchanged. That is right for text that is already
normalised -- which is what a bundler emits, and why it has survived -- and wrong for text that
is not.

What was fixed alongside this: a form outside `NFC`, `NFD`, `NFKC`, `NFKD` is a `RangeError`, and
the form is `ToString`'d on the machine so an object naming a form is accepted and one whose
`toString` throws does so. Answering the string for `normalize("XYZ")` told a caller its text had
been normalised into a form that does not exist, which is worse than not normalising.

Doing the rest needs Unicode decomposition and composition data, which this crate does not carry
-- a dependency decision (`unicode-normalization` or equivalent) rather than a bug fix, and one
that has to hold for the C backend too or the two engines will disagree about what a string is.
That is why this is filed rather than half-done: a partial normaliser is a silent wrong answer of
exactly the kind the backend refuses elsewhere.
