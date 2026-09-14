# Strings are UTF-8, so a lone surrogate becomes U+FFFD

Status: wontfix · Severity: medium (ADR 0022 accepted indefinitely)

    String.fromCharCode(0xD800).charCodeAt(0).toString(16)   // "fffd", node "d800"
    ("a" + String.fromCharCode(0xDC00)).charCodeAt(1)         // 0xFFFD, node 0xDC00
    "\uD800".charCodeAt(0)                                    // parsed as U+FFFD + "d800"

`RegExpData`, the lexer and every string value store Rust `String`, which is UTF-8 by
construction and cannot hold an unpaired surrogate. JavaScript strings are sequences of UTF-16
code units and *can*. Every route to one -- a `\uD800` escape, `fromCharCode`, `fromCodePoint`,
a slice through an astral pair -- silently substitutes U+FFFD instead.

Silent substitution is the failure mode: the program believes it built the string it asked for.

## What it blocks

`String.prototype.isWellFormed` and `toWellFormed` (ES2024) are absent, and were implemented and
then **backed out** rather than shipped. They exist precisely to detect and repair lone
surrogates, so on UTF-8 storage `isWellFormed()` can only ever answer `true` and `toWellFormed()`
can only ever be the identity:

    String.fromCharCode(0xD800).isWellFormed()   // would answer true; node false

Answering `true` there is a wrong answer in the one case the method is for, and this engine
prefers a missing member -- which fails by name -- over a member that answers wrongly. Roughly 32
rows in `built-ins/String`, plus the 18 in `fromCharCode`, wait on the representation rather than
on the methods.

## What fixing it means

Storing WTF-16 (or UTF-16 with a validity flag) for string values, and teaching every place that
converts to and from Rust `String` about the difference. That is a representation change of the
same order as the array storage one, and it touches both engines -- the C prelude stores
`kiln_text` bytes on the same assumption.

## Not a real-usage blocker

Well-formed text is unaffected: `"a😀b"`, astral characters, every ordinary string round-trips
exactly. A program only notices if it deliberately constructs a half surrogate, which is rare
outside encoder tests and fuzzers.
