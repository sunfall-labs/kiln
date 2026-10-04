# Object allocation, 2026-10-04

The paired native object-literal workload improved from 5.0625 ms to 4.4375 ms per warm iteration, a 12.3% reduction. Bun 1.4.2 took 0.0188 ms, leaving native about 236 times slower on this allocation-heavy microbenchmark. Every process returned checksum `600130000`.

| Runtime | Warm medians, ms | Median, ms |
| --- | --- | ---: |
| Native before | 5.125, 5.0625, 5.0625 | 5.0625 |
| Native after | 4.4375, 4.4375, 4.4375 | 4.4375 |
| Bun | 0.0187988, 0.0190430, 0.0187988 | 0.0187988 |

The candidate explicitly initializes each object's external-owner pointer and avoids finalizer cleanup for objects with no separately owned side storage. The owner change fixes a reproduced poisoned-memory initialization defect. This comparison includes both changes and does not isolate their individual timing effects. Objects with owned properties, dictionaries, sealed state, slots or elements retain their existing cleanup.

The existing harness separates first-call and warm execution and calibrates batches against clock resolution. These values are the median of three fresh-process warm medians, with runtime order alternating. All agents held compilation and tests during measurement. Compilation, startup and module setup are excluded. [Individual records and identities](object-allocation-2026-10-04.json) retain all nine samples.

The candidate's backend object suite passes 82 tests, and four affected native probes match under allocation stress. The integrated routine gate remains required before landing. These are JavaScript microbenchmark timings; they do not measure direct-TypeScript Effect coverage or its scheduler.
