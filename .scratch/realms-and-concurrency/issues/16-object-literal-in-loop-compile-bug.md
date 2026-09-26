# 16. `{ code: i * 3 }` inside a loop fails to compile natively

Status: ready-for-agent

Found while writing the concurrency benchmark (issue 13): an object literal whose property value is
an arithmetic expression on a loop variable, inside a loop, produces C that uses a temporary it
never declared. `bench/concurrency/js/common.mjs` works around it with `(i * 3) | 0`. Not a
concurrency bug, but a native build that fails on ordinary code.

Reduce it to a minimal probe in `conformance/native/`, fix the emitter, and remove the workaround.

## Acceptance

- The probe matches the interpreter in `scripts/native.sh`; the benchmark workaround is removed.
