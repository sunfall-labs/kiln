# Workspace layers

Crate directories declare dependency direction ([ADR 0066](../docs/adr/0066-crate-layers-and-dependency-direction.md)).

| Directory | Rank | Responsibility |
| --- | --- | --- |
| `foundation/` | 0 | Shared vocabulary and allocation-free algorithms |
| `semantics/` | 1 | Engine-independent language/platform semantics and contracts |
| `syntax/` | 2 | Parsing and owned HIR |
| `host/` | 2 | Realm host runtime |
| `compile/` | 3 | Bytecode, module graphs, program analysis, query engine |
| `interp/` | 4 | Interpreter and module evaluation |
| `native/` | 4 | C emission/runtime, provider implementation, native linking |
| `tools/` | 5 | CLI, daemon, conformance tools, benchmark driver |

Normal workspace dependencies may point to the same rank or downward. The two engine directories
must not depend on each other. Dev-dependencies may cross these boundaries for differential tests.
`mbx test -p kiln-test-selector --test layers` checks actual workspace metadata; no crate allowlist
needs updating when adding a crate.

`kiln-module-graph` builds graphs; `kiln-module-eval` executes them. Backend-neutral analyses live
in `kiln-graph`; liveness reports neutral retained instruction/function counts, not backend byte
sizes. The query engine receives its summary query from its caller rather than depending on an
emitter.

The native number ABI and vendored conversion sources live in `kiln-native-provider-contract`.
`kiln-number-core` remains foundational; `kiln-native-provider-ffi` owns the provider implementation.
Benchmark driver Rust code lives in `tools/kiln-bench`; workloads and results remain under `bench/`.
