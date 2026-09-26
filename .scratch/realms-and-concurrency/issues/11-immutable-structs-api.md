# 11. Immutable shared structs: the API

Status: needs-triage
Blocked by: 08

Decide, in an ADR, how a program puts data into the shared immutable region and what it gets back:
the built-in (a kiln extension under ADR 0068), whether values are deeply frozen objects or a
distinct kind, how they cross `postMessage` (by reference, never copied), and how they relate to
TC39 proposals in the same space (Records and Tuples, Shared Structs) so kiln does not ship a
conflicting shape.

## Acceptance

- An accepted ADR, and the built-in implemented against 08.
