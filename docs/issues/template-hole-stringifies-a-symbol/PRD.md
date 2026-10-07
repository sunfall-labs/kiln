# PRD: Template Hole Evaluation Disallows Symbol Coercion

Status: wontfix
Branch: `none`

## Problem Statement

ECMA-262 §13.2.8.3 specifies that template literal expressions undergo `ToString(sub_expr)`, which throws a `TypeError` on Symbol values. Kiln compiles holes via `String(expr)`, which converts symbols to `"Symbol(...)"` instead of throwing.

## Rationale for Wontfix

`language/expressions/template-literal` is 114 pass / 0 fail. Fixing this requires either a new `ToString` instruction across both backends or emitted symbol guards on every template hole, imposing runtime performance penalties across hot string interpolation for 0 Test262 gain.
