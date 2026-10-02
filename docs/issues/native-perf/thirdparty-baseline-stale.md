# The thirdparty timing baseline is 179 commits stale, and rxjs fails it on main

**Role:** implementer. **State:** open. **Found:** 2026-09-07, gating the Phase 6 work.

`mise run gate`'s "thirdparty baseline" step compares `bench/results/thirdparty-baseline.json`
against a fresh timing of each bundle. That file was last written by `9f5fb337`, **179 commits
ago**, and the engine has moved a long way in both directions since:

| bundle | baseline | main today |
| --- | --- | --- |
| acorn | 19.41 ms | 11.08 ms |
| marked | 9.68 ms | 6.15 ms |
| postcss | 7.58 ms | 4.88 ms |
| semver | 6.36 ms | 4.46 ms |
| **rxjs** | **16.48 ms** | **20.64 ms** |
| mustache | 1.83 ms | 3.13 ms |
| uuid | 2.00 ms | 2.79 ms |

**The step fails on `main` itself**, on rxjs (+25.2%, just over the 25% threshold). Measured on
`4976eb73` with the script's own five passes.

Two separate things are tangled here and both want doing:

1. **rxjs really is slower than it was**, by about a quarter, and nobody has looked at why. It is
   the one bundle in the set that is dominated by real work rather than process startup (20 ms
   against everyone else's 3-6), so it is the one whose number means something.
2. **The baseline should be refreshed** so the step means something again. Refreshing it first
   would bake the rxjs regression in and lose the only record that it happened, so investigate
   rxjs first, then refresh in a commit that says what moved and why.

The small bundles are 3-6 ms end to end, most of which is process startup, and a single pass of
one moves by half a millisecond run to run. Two runs of the same binary here disagreed by 29% on
mustache and 18% on uuid. Whatever replaces the baseline should either time enough passes to
make those stable or stop gating bundles whose runtime is mostly `execve`.
