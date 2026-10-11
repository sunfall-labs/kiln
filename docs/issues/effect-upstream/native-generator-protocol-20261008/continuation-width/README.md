# Continuation-width validation (2026-10-08)

Source `8116b52e7451ba129b16f84fc07e7bec83d8ec35` widens the reaction and microtask continuation fields from two-bit fields to `uint8_t`. Modes 4, 5 and 6 previously truncated to 0, 1 and 2. The isolated C roundtrip test passed; a clean bounded release completed in 251.1 seconds. The selected release receipt and exact probe streams are retained here.

`async-generator-method-dispatch.mjs` matched (1 matched, 0 refused, 0 diverged). The original r59 probe still diverged in `resultResolutionAllowsReentrantNext` with a native undefined-property TypeError. This is diagnostic progress, not complete generator or Effect acceptance. The subsequent repair targets ordinary objects' implicit Object.prototype accessor lookup, which passed the hidden property store as receiver.

Both retained probe runs ended normally with unchanged source, CLI and provider pins and observed process-group cleanup. Full native and interpreter Effect replays, stress/realm probes, the final gate and benchmark comparisons remain required. No push has succeeded during the DNS outage.
