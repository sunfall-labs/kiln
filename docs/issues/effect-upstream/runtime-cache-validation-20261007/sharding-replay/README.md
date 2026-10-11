# Fresh native Sharding replay

The frozen combined candidate built the original Sharding entry successfully
in 316.724 seconds. Its executable SHA-256 is
e230567c928346b8a7d916a430796c794e5615f2867c8e11b2f1c8e1ce5ff205.
The quiet prebuilt replay completed all 66 original cases with 62 passed,
4 failed and no skips, in 47.572 seconds. All failures reached the unchanged
5-second case deadline; no other assertion failure was recorded. The exact
case IDs, modes, hashes and official mapping verdict are in the retained report.

The timeouts concern active claims after an entity defect, an uninterruptible
request after restart, an active stream after an acknowledged chunk, and reset
redelivery during an asynchronous storage read. These are remaining work.
This single result does not prove a stable timing gain over earlier candidates.

The stage supervisor did not implement an outer process-group timeout; the
build nevertheless completed normally. The corrected prebuilt replay helper
uses a process-group timeout and termination/reaping path. Both original raw
logs and receipts are retained, with no deadline or assertion changes.

This is one 66-case file, not a complete 322-file native or interpreter run.
Full Effect acceptance, Bun benchmarks and the complete gate remain required.
