# Date scalar field work

Cron invokes the shared TypeScript Date getters repeatedly. Every getter previously called `dateParts`, which computed all calendar and clock fields and allocated an object even when only seconds or milliseconds were requested. The candidate computes clock/day fields from the timestamp, and calendar fields from `civilFromDays` without the extra full-field object. Setters and formatters retain their existing shared conversion path.

The oracle compares the installed TypeScript implementation to each host's original Date methods over 2,023 timestamps, including invalid dates, negative values, exact day boundaries and both TimeClip endpoints. Node 26.8.1 and Bun 1.4.2 each match 18,207 field results and reject all nine incompatible receivers. This host check proves arithmetic for those inputs; kiln execution and the pinned Date Test262 subtree remain pending. No Effect performance improvement is claimed yet.

The oracle exposed an existing negative-zero defect at exact negative day boundaries. Normalizing the shared modulo helper's zero to positive zero fixes that field result, including weekdays at negative multiples of seven. The native probe rejects negative-zero fields explicitly, then compares its deterministic count/checksum with Node.
