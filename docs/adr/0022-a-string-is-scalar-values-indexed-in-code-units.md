# ADR 0022: A string is Unicode scalar values, indexed in UTF-16 code units

## Status

**Accepted**, and it records a limitation rather than a preference. The alternative is a
representation change large enough to be its own project; this fixes the inconsistency that made
the limitation dangerous and names the escape route.

## Context

A JavaScript string is a sequence of UTF-16 code units, and the specification does not require
those units to be well formed: `"a\u{1F600}b"[1]` is a *lone high surrogate*, a value with no
Unicode scalar behind it. Every position-taking method counts in those units, which is why
`"a😀b".length` is 4 and not 3.

Kiln stores a string as a Rust `String`, which is guaranteed well-formed UTF-8 and therefore
**cannot hold a lone surrogate at all**. That is not a bug in the interpreter; it is a property of
the type the value carries.

The bug was that the interpreter had not decided which unit a *position* was measured in, and
different methods had answered differently. `length`, `charAt`, `substring`, `indexOf` and `[i]`
counted UTF-16 code units. `slice`, `at` and `lastIndexOf` counted characters. So for any string
containing an astral character:

```js
const s = "a😀b";
s.length          // 4
s.slice(0, s.length)  // "a😀b" via characters -- but s.slice(0, 3) was the whole string too
s[1]              // one code unit
s.at(1)           // one *character* -- a different position with the same index
```

Two spellings of "the character at 1" gave different characters, and `slice(0, n)` walked past the
end of what `length` had promised. Both silently. No probe caught it, because no probe sliced a
string containing an emoji -- which is exactly the shape of bug `conformance/differential` exists
to find and had not been pointed at.

## Decision

**A position is a UTF-16 code unit, everywhere.** `length`, `[i]`, `charAt`, `at`, `charCodeAt`,
`codePointAt`, `slice`, `substring`, `substr`, `indexOf`, `lastIndexOf`, `split` and `replace` all
count the same thing. This is the language's rule and it is now applied without exception.

**An unpaired surrogate becomes U+FFFD.** When a position lands inside a surrogate pair, the half
that results has no scalar value, so it cannot be stored. `String::from_utf16_lossy` substitutes
the replacement character, and every one of these methods goes through the same conversion, so
they at least agree about what they cannot represent.

This is a **deliberate divergence from node**, and the only one in the string surface:

```js
"a😀b".slice(0, 2)   // node: "a\ud83d"   kiln: "a\u{FFFD}"
```

It is recorded here rather than hidden because the differential corpus would otherwise report it
as a failure forever, and because a reader who hits it deserves to find this page rather than
guess.

## Consequences

- The invariant a program actually depends on now holds: `s.slice(0, s.length) === s`, and
  `s[i]`, `s.charAt(i)` and `s.at(i)` name the same position. Truncation, scanning and slicing
  loops are correct for astral text, which they were not.
- A program that deliberately manipulates lone surrogates -- a UTF-16 codec, a fuzzer, a test of
  ill-formed input -- gets the wrong answer, quietly. That is the residual risk and it is not
  mitigated by anything here.
- Round-tripping ill-formed UTF-16 through kiln is lossy. `s === s.slice(0, 2) + s.slice(2)` is
  false when the cut falls inside a pair.
- The native backend inherits the same rule for free, because it re-encodes from the same UTF-8
  bytes; the two engines agree with each other, which is what `scripts/native.sh` checks.

## The escape route

The fix is a string representation that can hold ill-formed UTF-16. Two candidates:

- **WTF-8** -- UTF-8 generalized to encode unpaired surrogates. A superset of UTF-8, so every
  well-formed string keeps its current bytes and only the ill-formed cases cost anything. This is
  what Rust's own `OsString` uses on Windows for exactly this problem.
- **`Vec<u16>`** -- store what the language stores. Correct by construction, and it makes every
  byte-oriented operation (host I/O, JSON, source text) pay a conversion instead.

WTF-8 is the better fit: the common case stays free, and the boundary to the host stays bytes.
Either way the change is `Value::String(String)` becoming `Value::String(Rc<JsString>)` and every
site that pattern-matches it, which is why it is not being done alongside a string-method slice.

Until then, this ADR is the answer to "why did my emoji turn into a question mark".
