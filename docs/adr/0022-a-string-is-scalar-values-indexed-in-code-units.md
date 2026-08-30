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

### Naming it, so it is not re-derived

The paragraphs above describe the route without naming anything, which is the state in which
somebody works it out again from scratch. Checked 30 August 2026:

**The value space has a name, and a production precedent.** Servo's `DOMString` documents exactly
this space -- "a DOMString has the same value space as a JavaScript String, i.e., an array of
16-bit *code units* representing UTF-16, potentially with unpaired surrogates present (also
sometimes called WTF-16)". Read it before designing anything here, but read it for the right
reason: Servo hit this problem first and **landed on the same answer this ADR does**, replacing
the unpaired surrogate with U+FFFD at the JavaScript-to-Rust boundary, and offers "the lack of
crash reports about this issue" as its evidence that the trade holds. So the precedent that
matters is precedent for the Decision, not for the escape route. That is worth knowing in both
directions: it is the strongest available evidence that U+FFFD is a defensible resting place
rather than a stopgap, and it means nobody should cite Servo as a reason to change.

**WTF-8 has an implementation.** The `wtf8` crate provides `Wtf8` and `Wtf8Buf` mirroring `str`
and `String`, plus a `CodePoint` type for a value that may be a lone surrogate. U+D800 encodes as
the three bytes `ed a0 80` -- the ordinary three-byte UTF-8 pattern applied to a code point UTF-8
forbids, which is the whole trick and the reason well-formed text keeps its current bytes. The
shape is the point: `Value::String(String)` becoming `Value::String(Wtf8Buf)` is a type
substitution at the boundary rather than a redesign of it. `wtf8-rs` and `omgwtf8` cover the same
ground if the primary one proves unsuitable.

Take their maturity into account rather than around it. `wtf8` is at 0.1.0, last published April
2021; `wtf8-rs` at 1.1.0, December 2021; `omgwtf8` at 0.1.0, 2018. None is actively maintained.
Rust's own `std` carries this encoding internally for `OsString` on Windows and does not export
it. So the real choice is between depending on an unmaintained crate and vendoring roughly a
thousand lines of a frozen, well-specified encoding -- and vendoring is not obviously the worse
half of that. Nothing here needs deciding now; it needs to not be rediscovered.

### This is a recorded route, not a decision

Nothing above changes the Decision. A string is Unicode scalar values indexed in UTF-16 code
units, an unpaired surrogate is U+FFFD, and `"a\u{1F600}b"[1]` still has no representation.

The cost is the reason, and it is worth being concrete about *why* it is large rather than
repeating that it is. The change touches every string operation in **both** engines.
`kiln-interpreter` pattern-matches `Value::String(String)` and reaches for `String`'s inherent
methods throughout; `kiln-backend-c` re-encodes from those same UTF-8 bytes into emitted C, which
is precisely the mechanism that makes the two engines agree today (see Consequences). Moving the
representation means moving both, in step -- and while it is in flight the differential corpus
cannot tell you that one has diverged from the other, because both have moved and
`scripts/native.sh` only checks them against each other. That is what "a representation change
large enough to be its own project" in the Status line means, stated concretely.

What writing this down buys is narrow and real: it stops the next person hand-rolling a bespoke
WTF-8 in the middle of fixing one string method, which is how a representation change gets made
by accident and half-made. It does not pre-approve the change and it does not make the change
smaller. Whoever takes it still has to make the argument on evidence that does not exist yet -- a
real program that needs lone surrogates, rather than the hypothetical one in Consequences.

Until then, this ADR is the answer to "why did my emoji turn into a question mark".

## Sources

Checked 30 August 2026.

- `wtf8`: <https://crates.io/crates/wtf8>, <https://docs.rs/wtf8> (the `Wtf8` / `Wtf8Buf` /
  `CodePoint` API); the encoding itself: <https://simonsapin.github.io/wtf-8/>
- Alternatives: <https://crates.io/crates/wtf8-rs>, <https://crates.io/crates/omgwtf8>
- Servo's `DOMString`, for the WTF-16 vocabulary and for its U+FFFD resolution:
  <https://doc.servo.org/script/dom/bindings/str/struct.DOMString.html>
