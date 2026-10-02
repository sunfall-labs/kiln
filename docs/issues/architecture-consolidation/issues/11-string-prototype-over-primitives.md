# 11. `String.prototype` in TypeScript over primitives

Status: ready-for-agent
Blocked by: 10

The first core ECMAScript family. `String.prototype.padStart` alone is `kiln-interpreter/src/lib.rs`
near line 38866 and `kiln-backend-c/src/runtime/strings.c` near line 1245.

Write the methods in `kiln-builtins` over representation primitives (code-unit length, code unit
at an index, build a string from units or slices). All coercion and error selection lives in
TypeScript. Delete the interpreter `Native::String*` arms and the C `KILN_C99_METHOD_*` string
cases in the same change.

Measure before and after with `mise run bench` for the string benchmarks. Where the native build
loses more than the recorded budget, add a *measured* primitive (ADR 0065) with the workload and
both numbers in its row, not a revived C method.

## Acceptance

- No `String.prototype` member has an engine implementation other than declared primitives.
- The duplicate count falls by the family's size.
- Test262 `built-ins/String` chunks do not regress (`scripts/test262.sh --path built-ins/String`).
