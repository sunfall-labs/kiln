# Current production Hash interpreter profile

This diagnostic uses the production CLI SHA-256 `cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2`, which passed the routine gate on source `f53e22c6` and was pushed in milestone `ab63eaee`. It excludes the pending Schema selector and instantiation-tier changes.

The original TypeScript Hash entry and harness ran directly, with the Node adapter, wall clock, one runtime worker and a 4 GiB address-space cap. All 25 original cases were recorded: 24 passed and the persistent/in-place HashMap edit law hit its unchanged 5,000 ms deadline. Profiling and concurrent bounded compiler work make this a diagnostic outcome, not an acceptance run or a performance comparison. The target and sampler both terminated; their full output and exact command/identity receipts are retained.

The failing case window runs between its predecessor's completion marker and its own timeout marker: 26.151 seconds and 2,617 worker samples. Leaf attribution is distributed: interpreter execution 545 samples (20.8%), Value clone 134 (5.1%), Value drop 101 (3.9%), StringUnits-to-str equality 85 (3.2%), and ObjectData property lookup 76 (2.9%). These self counts do not attribute nested callers or prove an optimization will fix the timeout. Library-qualified libc symbols remain library symbols; generic libc labels are not generated kiln functions.

The archive includes the raw profile, symbol sidecar, case markers, analysis script/output, source-address results and input digests. The analyzer selects the actual worker thread and bounds attribution by the original case markers. Source-address resolution uses this exact CLI's debug information. The entry remains the pinned upstream workload; no assertion, skip or deadline was changed.
