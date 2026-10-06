The collector now shares each function's immutable suspension-point/local-liveness table through `Rc`, rather than cloning every HashMap and live-slot set for every suspended generator visit. The cache retains its weak program identity check and contains only instruction/slot integers; it adds no JavaScript roots. Collection scheduling and liveness decisions are unchanged.

Validation: `mbx test -p kiln-interpreter --offline --lib --jobs 1` on Rust 1.97.1 passed 285 tests, failed 0, ignored 1. The full log includes generator local-cycle collection, live-iterator retention, WeakMap ephemerons, and automatic collection/job keepalive checks. Formatting and diff checks passed. The first attempted namespace filter selected zero tests; its log is retained as an unsuccessful selection, not test evidence.

No Effect throughput improvement is claimed yet. The original workflow replay/profile is still needed to assess collector cost and unchanged case deadlines.
