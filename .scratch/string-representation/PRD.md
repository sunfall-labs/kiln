# PRD: Lone Surrogate Representation in Runtime Strings

Status: ready-for-agent
Branch: `feat/utf16-surrogates`

## Problem Statement

Strings in Kiln are stored as UTF-8. A lone surrogate code unit (such as `0xD800` or `0xDC00` from `String.fromCharCode(0xD800)`) is not valid UTF-8 and was replaced with `U+FFFD`. JavaScript strings are sequences of 16-bit code units (WTF-8 / loose UTF-16).

## Impact

- `String.fromCharCode(0xD800).charCodeAt(0)` answers `0xFFFD` instead of `0xD800`.
- Blocks string surrogate handling and multiple Test262 String built-in tests.

## Proposed Architecture

Adopt WTF-8 (Wobbly Transformation Format) storage or explicit 16-bit storage for strings containing lone surrogate code units.
