# Reference workloads

`small/` and `medium/` are the fixture projects `kiln-bench` measures against (design.md §13).
They are **generated, not authored** — `kiln-bench` renders them from a fixed seed and rewrites
those two owned directories whenever their manifests no longer match. The generator lives in
`crates/tools/kiln-bench/src/workload.rs`.

`hello.ts` and `modules/` are small, hand-authored public CLI smoke fixtures. The latter exercises
exact relative resolution, dependency-first side effects, and a named scalar import.

| workload | modules | shape                                                    |
|----------|---------|----------------------------------------------------------|
| `small`  | 8       | 3 layers, 2 imports per module — the "handful of modules" case |
| `medium` | 320     | 8 layers, 4 imports per module — a realistic import graph |

Each module is TypeScript with a stable set of exports, a few module-private helpers, and
imports drawn from strictly earlier modules (which is what makes the graph acyclic by
construction). Modules are distributed across `src/<area>/` directories by layer so the tree
looks like a project rather than one flat folder.

To materialise them without running the benchmark:

    cargo run -p kiln-bench -- gen

Generation is deterministic: the same commit produces byte-identical files, which is the only
reason timings from two different runs are comparable at all. `.kiln-bench-manifest` in each
workload directory records the fingerprint that was written.

They are not committed — regenerating them is cheaper and more trustworthy than reviewing a
few hundred generated files in a diff.
