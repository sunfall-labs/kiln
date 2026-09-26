# Interpreter

Layer 4a (`interp`). Owns bytecode execution, JavaScript values and objects, realm-local
intrinsics, and the interpreter's host adapters. Public entry points and value types remain
available from the crate root.

## Source map

- `vm/` holds execution, calls, object operations, host integration, and intrinsic invocation.
  Call/eval entry, async generators, module loading, async context, error handling, closures,
  process/arguments objects, DataView, Date, weak references, and disposal have separate modules.
  `construction.rs` implements generic construction; `constructors.rs` dispatches built-in
  constructors, with collection and Promise construction in their own modules.
- `intrinsics/` holds pure built-in dispatch.
- `object_model.rs`, `object_storage.rs`, `object_helpers.rs`, and `properties.rs` hold object
  representation, storage, prototype traversal, and intrinsic property descriptors.
- `runtime_value.rs`, `value.rs`, and `value_helpers.rs` hold values, conversions, and formatting.
- `binary_data.rs`, `date.rs`, and `serialization.rs` hold binary views, date operations, and
  serialization. `intrinsic_helpers.rs` and `builtin_members.rs` hold shared built-in operations
  and member selection.
- The named platform modules (`http.rs`, `fs.rs`, `stream.rs`, and peers) implement host adapters.
- `tests/` groups regression tests by values, objects, language, host services, and runtime.

Module boundaries do not change realm identity, property semantics, scheduling, or public APIs.
The VM files remain cooperating `impl Vm` modules declared at the crate root; they preserve the
old internal visibility rather than pretending to be independent services. Narrower internal
interfaces and explicit imports are a separate follow-up from these behavior-preserving moves.
Graph construction belongs to `kiln-module-graph`; graph evaluation and filesystem-backed module
loading belong to `kiln-module-eval`. C emission and provider implementation do not belong here.

Run `mbx test -p kiln-interpreter --offline`. The source scan in the test suite recursively covers
implementation modules so moving an implementation does not remove it from the internal-slot
checks.
