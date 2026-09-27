# Module graph

Layer 3 (`compile`). Owns entry-rooted graph construction, export resolution, closed-world
classification, immutable graph snapshots, and live-graph reduction.

The graph contains source identities and bytecode, never interpreter values or realm state.
Evaluation and dynamic-import materialization belong to `kiln-module-eval` in layer 4a.
The native engine consumes graph construction without building the interpreter.

Run `mbx test -p kiln-module-graph`. Integration tests may depend on the evaluator; normal
dependencies must follow ADR 0066.
