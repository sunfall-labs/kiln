# Live namespace resolver: combined crate validation

Integrated resolver commit `779f0932` (Luna source `b083419739fd0cf50b230b02849d22e29475b035`). CommonJS consumer repair and stronger public parity fixture: `ac0099b7`.

`mbx test -p kiln-interpreter -p kiln-module-eval --offline -- --test-threads=1` completed with 271 interpreter passes, one existing ignored test, and 32 module-evaluation passes: 303 passed, zero failed. Raw complete output is retained here; no verdict is inferred from empty doc-test targets. Formatting check passed.

Coverage includes live descriptors before any Get, namespace inheritance, ordered/numeric and duplicate names, enumerable copying and JSON, collection of replaced snapshots, reachable/unreachable namespace-cell cycles, and existing CommonJS namespace snapshot/identity cases. Namespace-only metadata is boxed; ordinary objects do not carry two extra vectors. Published export slots no longer retain old values; public data readers resolve their cells through an owned-value path, including fs/HTTP options and UTF-16 CommonJS snapshots.

The first combined release build found the removed bulk-refresh API in the external CommonJS loader. That consumer was migrated rather than restoring snapshot refresh. A second loader path read raw placeholders and was also migrated, preserving exact UTF-16 lookup.

Combined final CLI, native public parity, pinned namespace Test262 and real-clock Effect performance checks are still pending. This evidence is focused crate validation, not the gate or full Effect acceptance.
