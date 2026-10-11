# Fresh native Cluster diagnostic profile

This is diagnostic evidence, not a passing acceptance run. All 47 original
cases and modes were recorded; 46 passed and the parent interrupt-retry-budget
case reached its original 5-second timeout. A separate earlier quiet replay
was 42/47. The differing outcomes do not establish a performance improvement.

The 49 Hz per-thread USER CPU sampler recorded 1,687 samples with zero lost,
unmapped or overrun samples. Counter/RUSAGE coverage was 1.0041 and sample-period
coverage was 0.9635. Timestamped executable maps retain actual image identities.
No callchains were recorded; function counts are leaf samples only.

The original executable was stripped. A symbol-preserving relink used the exact
frozen CLI, source entry, provider and native object cache: all object categories
were cache hits, C compile time was zero. C generation still took 57.9 seconds;
total invocation time was 64.9 seconds. All four PT_LOAD headers and executable
bytes matched exactly. The only loadable differences were 26 bytes comprising
ELF section-table metadata and the GNU build-ID in a non-executable segment.

Only sized defined STT_FUNC ranges were accepted for attribution, with no
nearest-symbol fallback: 1,505 of 1,518 native samples resolve uniquely, 13 fall
in symbol gaps, and none are ambiguous. Every one of the timeout-window's
213 native samples resolves uniquely. Its 235 total samples include nursery
lookup (11), key interning (10), own-property-kind lookup (8), own-property read
(7), keyed write (7), boundary conversion (7) and method dispatch (7). These
small leaf counts support investigation of shared runtime overhead; they do not
identify a single dominant inclusive bottleneck.

The raw profile, original case output, mapping validation, exact executable
segment comparison and function-attribution rows are retained here. ELF files
remain in the private validation workspace; their SHA-256 identities are in
the receipts. Full native/VM suites, gate and Bun comparisons remain required.
