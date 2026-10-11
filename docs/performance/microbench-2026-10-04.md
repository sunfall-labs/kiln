# Repository microbenchmarks, 2026-10-04

A dated record from one Linux machine; `mise run bench` reproduces the table on yours.

All 22 repository microbenchmarks completed under Node, Bun, the interpreter and native, with no reported checksum divergences. The production compiler is the immutable `560e7ccc` binary validated at `e872e3a0`; all native binaries use `native-release`. Builds finished before the measurement window, and no other builds or tests ran during timing.

The repository harness separates first-call timing from warm steady state. The table reports steady-state milliseconds per workload iteration, using the median of three process samples. Native compilation and process startup are excluded. The interpreter uses external timing with its matching measured floor because its virtual clock cannot time synchronous work. [Machine-readable results](microbench-2026-10-04.json) retain all four runtimes, floors, compiler/harness/source/binary digests and the measurement conditions.

Native is faster than Bun in 6 rows and within 10% in 8. The median per-row native/Bun ratio is 2.32; rows have different workloads and this is not a weighted throughput score. Numeric workloads are competitive; object allocation, polymorphic classes and Buffer concatenation remain substantial performance debts.

| Workload | Node ms | Bun ms | Native ms | Native/Bun |
| --- | ---: | ---: | ---: | ---: |
| cpu/collatz.mjs | 478 | 421 | 192 | 0.46x |
| cpu/fib.mjs | 36 | 23.75 | 54 | 2.27x |
| cpu/frame.mjs | 0.0003204 | 0.0002594 | 0.0047 | 18.12x |
| cpu/loop.mjs | 54 | 53 | 46 | 0.87x |
| cpu/mandelbrot.mjs | 17.25 | 17.75 | 17.5 | 0.99x |
| cpu/nbody.mjs | 492 | 479 | 540 | 1.13x |
| non-io/array-functional.mjs | 8.375 | 1.719 | 1.875 | 1.09x |
| non-io/array-sort.mjs | 3.375 | 3.125 | 3 | 0.96x |
| non-io/class-poly.mjs | 1.078 | 1.078 | 30 | 27.83x |
| non-io/collections.mjs | 4.375 | 3.5 | 8.25 | 2.36x |
| non-io/json.mjs | 55 | 37 | 71 | 1.92x |
| non-io/multicore-parallel.mjs | 0.8438 | 0.1621 | 0.1719 | 1.06x |
| non-io/object-literal.mjs | 0.01904 | 0.0188 | 5 | 265.97x |
| non-io/object-spread.mjs | 0.7656 | 0.9219 | 0.2227 | 0.24x |
| non-io/parallel-map-large.mjs | 146 | 70 | 355 | 5.07x |
| non-io/property-miss.mjs | 0.04199 | 0.05176 | 0.7734 | 14.94x |
| non-io/regex.mjs | 6.25 | 0.5469 | 7.75 | 14.17x |
| non-io/string-ops.mjs | 13.25 | 7.25 | 18.75 | 2.59x |
| non-io/timer-create.mjs | 2.438 | 1.75 | 5.062 | 2.89x |
| io/buffer.mjs | 10 | 6.125 | 1133 | 184.98x |
| io/fs-promises.mjs | 11 | 5.375 | 4.562 | 0.85x |
| io/microtasks.mjs | 0.75 | 0.625 | 1.594 | 2.55x |

These are JavaScript microbenchmarks. The direct-TypeScript Effect comparison uses the pinned upstream tests and Bun separately; this table does not replace that inventory or establish whole-suite Effect performance.
