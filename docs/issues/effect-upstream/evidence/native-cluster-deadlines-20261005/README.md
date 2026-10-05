# Quiet native Cluster deadline baseline, 2026-10-05

All 47 original ClusterWorkflowEngine cases were recorded and exactly mapped to
the pinned official Node/Vitest oracle. Native passes 46; the sole failure is
`a parent stays suspended past the activity interrupt retry budget`, at its
original 5,000 ms deadline. No skips, missing records, mode mismatches, source
transforms or runner errors occur. The program and replay verifier exit 1 as
expected. This is a complete one-file negative baseline, not full acceptance.

The team stopped its build/test lanes before timed execution and held them until
the process completed. The file executes in 33.935 seconds. Compilation happened
separately and took 229.638 seconds (38.508 seconds emission, 188.774 compilation,
0.564 linking), with one native worker. These figures are baseline diagnostics,
not a final Bun benchmark comparison. External machine work was not controlled.

The source is the frozen clock candidate `faff5879`, CLI SHA-256
`e7a06733713ec8acfb7e3cc981ad78b1d881befa57b6d4ab455c3ed33f5139a7`,
before the newer Array/disposal/tier fixes. The native-release executable SHA is
`69094413515396f0ec465dc630143a737384ef7f0573e7c98582e08d0e4a48f4`;
compiler is installed Clang 22.1.8 targeting x86_64 Linux. The corrected harness
SHA is `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`
(`827129f2`). The report preserves all input hashes, commands, cases and exact
oracle comparison. The raw executable remains in owned scratch, outside Git.

The replay uses one runtime worker, an 8 GiB address-space cap and 1,800-second
outer process allowance. Explicit source deadlines remain unchanged. The
archived `replay.py` invokes the repository runner's execution, record-validation
and oracle-comparison functions; it checks records rather than trusting an exit
code. It expects the owned `.scratch/frozen-clock-native` layout from this
validation session. `build.log`, `native.log` and `report.json` retain the full
evidence. A combined final-source run remains required.
