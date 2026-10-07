# Dynamic source boundary evidence

The checked-source changes are exercised from this integration worktree through a standalone,
portable Cargo harness. Its dependencies point at this checkout, and the empty `[workspace]`
section keeps the scratch package outside the repository workspace. Run it from the repository
root with:

```sh
mise exec -- mbx run --manifest-path docs/issues/sota/dynamic-source/Cargo.toml --bin dynamic-main --offline
```

The harness callback prints UTF-16 units received by `EvalSource.source`. It exercises three
dynamic `eval` inputs built from `String.fromCharCode(0xd800)`: a raw surrogate inside a quoted
literal, an escaped `\\uD800` spelling, and a raw surrogate outside a literal. The checked boundary
rejects raw unpaired units before the parser and leaves valid escaped source on the ordinary parser
path. The current integrated output is in `integrated-dynamic-source-output.txt`.

The same frontend→IR→interpreter harness compiles and executes
`conformance/native/inferred-name-auto-accessor.ts` in this checkout. Its output is in
`integrated-auto-accessor-output.txt`; the initial reads must be `11 22`, followed by `101 202`
after the writes. This is a source-level run, so it exercises the frontend, compiler, property
keys, descriptors, and VM together.

The `historical-*.txt` files preserve evidence from the pre-integration harness. In particular,
the historical auto-accessor output began with `undefined 22`; it is retained as a labeled record
of the old VM behavior rather than as current evidence. `historical-node-probe-output.txt` is the
Node oracle for raw and escaped quoted strings, tagged-template cooked/raw contents, regexp source,
comments, invalid outside-literal input, Function source reflection, and low surrogates.
