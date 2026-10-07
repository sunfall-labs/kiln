# Remaining validation costs: prioritized investigation

Start: `e7591c17`, in an owned worktree. The preceding complete gate measured 386 s,
including 339 s for workspace tests, 261 s for native parity, and 99 s for full Test262.
Those stages overlap, so their times cannot be added.

The workspace raw log attributes 121.53 s to kiln-native, 73.99 s to kiln-cli,
46.61 s to kiln-daemon and 13.82 s to kiln-backend-c. Most other suites are below 3 s.
Optimizing small unit suites cannot materially change the gate.

Priority hypotheses, assigned to Luna agents:

1. Native development builds specialize the prelude by each program's retained method
   groups, multiplying C compiler work. Investigate sharing the compiled dev/check prelude
   without weakening release specialization or misreporting provider requirements.
2. Cache-hit builds still spend substantial time outside emission. The representative
   warm build takes 0.41–0.68 s despite 21–23 ms emission and no C compilation/linking.
   Benchmark scalar SHA-256 against the already-pinned maintained backend; preserve exact
   digests. Also measure reuse of the fixed bootstrap's immutable compiled code.
3. Rank the actual slow Test262 variants and improve the largest remaining semantic or
   representation costs. Preserve prototype/coercion behavior and instruction budgets.

The first native probe's cold stages were 767 ms emission, 1978 ms compilation and 125 ms
linking. These single-probe observations motivate work; they are not whole-group forecasts.
A complete native-parity baseline is being measured at eight workers in this worktree.

The objective is a substantial group reduction, not merely a handful of percent. Prefer
removing repeated work and sharing immutable artifacts. No cached test verdicts, smaller
stress repetitions, lowered baselines, or instruction-budget cuts. A final complete gate
will validate the integrated result once development is finished.
