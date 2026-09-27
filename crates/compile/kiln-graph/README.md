# kiln-graph

Layer 3 compile analysis. This crate owns backend-neutral SSA value graph construction,
verification, closed-world bytecode liveness, and facts derived from compiled programs.

The graph preserves bytecode control flow, source identity, and conservative dataflow facts used
by both compile-time analysis and native emission. Native emitters may consume these facts, but
this crate does not depend on a runtime engine or emitter. C layout, emitted byte size, and runtime
representation belong to the native layer.
