# 10. The parallel family

Status: ready-for-agent
Blocked by: 08

Add, on `Array.prototype` and `%TypedArray%.prototype`, marked as kiln extensions (ADR 0068), each
with the sequential method's semantics exactly:

| method | identical to | note |
| --- | --- | --- |
| `parallelFilter` | `filter` | |
| `parallelFlatMap` | `flatMap` | arrays only |
| `parallelSort` | `sort` | stable; for a comparator that is consistent, the order is exactly `sort`'s. For one that is not, `sort`'s order is implementation-defined, so the call runs sequentially unless the comparator is proven consistent |
| `parallelReduce(fn, combine, initial)` | none | no sequential equivalent: the tree over the input is fixed by length alone, and the interpreter applies the same tree, so the answer is deterministic in both engines (ADR 0069) |

Each is TypeScript in `kiln-builtins` over the parallel primitive (ADR 0065), with the sequential
method as its fallback.

## Acceptance

- Each method's probes in the 02 harness, at every parallelism level, identical to the sequential
  method (or, for `parallelReduce`, identical across engines and core counts).
