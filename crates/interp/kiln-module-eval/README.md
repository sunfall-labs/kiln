# Module evaluation

Layer 4a (`interp`). This crate executes linked module graphs in the interpreter, constructs
dynamic-import plans, and owns the realm-scoped graph loader and CommonJS evaluation cache.

Graph construction and export resolution belong to `kiln-module-graph` in layer 3. They do not
depend on this crate or on the interpreter. Callers import `ModuleGraphExecution` to execute a
graph; construction failures remain `ModuleGraphError`, while evaluation returns
`ModuleEvaluationError` and preserves the underlying execution error.

The graph's integration tests cover static linking, dynamic imports, CommonJS cycles, shared
realm identity, and Node host execution. Run `mbx test -p kiln-module-graph`.
