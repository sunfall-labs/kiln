# PRD: Concise Methods Lack Prototype Property and Reject [[Construct]]

Status: resolved
Branch: `fix/method-prototype`

## Problem Statement

Concise object methods (`{ m(){} }`) incorrectly had a `.prototype` property and could be invoked as constructors via `new obj.m()`. Per ECMA-262 §15.4.3, method definitions do not have a `[[Construct]]` internal method and have no `.prototype` property.

## Resolution

- Stripped `prototype` property creation from concise method definitions.
- Marked concise method function objects non-constructible; `new` throws `TypeError`.
