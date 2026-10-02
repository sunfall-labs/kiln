# 16. `{ code: i * 3 }` inside a loop fails to compile natively

Status: resolved

Found while writing the concurrency benchmark (issue 13): an object literal whose property value is
an arithmetic expression on a loop variable, inside a loop, produces C that uses a temporary it
never declared. `bench/concurrency/js/common.mjs` works around it with `(i * 3) | 0`. Not a
concurrency bug, but a native build that fails on ordinary code.

Reduce it to a minimal probe in `conformance/native/`, fix the emitter, and remove the workaround.

## Acceptance

- The probe matches the interpreter in `scripts/native.sh`; the benchmark workaround is removed.

## Comments

**2026-09-27 — branch `work/messaging-finish`, `eb90bcff`: resolved.**

It was not the object literal. `emit_binary_ints` writes a sum, difference or product of two
integers that is not proven to stay an integer into the value's double twin (`d<N>`), but the
liveness seed in `emit_graph/body.rs` marked every such result integer-only, so the twin was
declared only when something else read it. `{ code: i * 3 }` in a loop is a product read by
nothing but its box, and under `--profile native-release` (the only profile whose typed bodies
reach that lowering; the development profile compiled it) the C named an undeclared `d16`. The
seed now marks a quotient, a power, and a sum, difference or product outside `ints` as a double
result.

- Probe: `conformance/native/object-literal-arithmetic-in-loop.js` (release profile directive):
  the original `i * 3`, plus a product, sum and difference whose values pass 2^31 and 2^53, so
  the literal is shown to hold the double rather than a truncated integer. Interpreter and
  native print node's bytes.
- `bench/concurrency/js/common.mjs` builds its categories with `{ code: i * 3 }` again.
