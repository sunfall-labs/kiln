# `Object` statics missing or unreachable natively

Status: resolved

Resolved via ADR 0043 (one-table builtin surface). Verified that `Object.getOwnPropertySymbols` and `Object.getOwnPropertyNames` (including on builtin constructors) work natively and match node.
