# Native Pool verification with allocation census

On 2026-10-05 the frozen integration CLI passed the unchanged pinned Effect `Pool` test
file in the native engine: 47 discovered, 47 passed, no skips. All 47 cases mapped to the
official Node/Vitest reference, with no inventory, mode, or outcome mismatches.

The source pin is Effect commit `67ba4e46a11ccda0b6761578bfd22c04ae00167`; the `Pool` source
SHA-256 is `a219193dd30dc547f7364a723e2682d653ab824ccbda816b48b1a9d5bfb539f8`. The frozen
integration checkout was at `4b5e6c9cc68661f19599ec02ed665e9a47379270`. Its CLI SHA-256 is
`734d4a745f4beea64662d94fb87dcb82fd897600ed584056bff1da30941ba70a` and it reported protocol
41. The native-release provider manifest SHA-256 is
`4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; its archive SHA-256 is
`61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d` (provider ABI 82).

The run used one compiler/runtime worker and the upstream test source, Vitest mapping, and
per-case budgets without modification. `--timeout 1800` was the runner's outer file/build safety
limit. An earlier run with a 600-second outer limit timed out while compiling, before registering
or executing any cases; that attempt is retained separately and is not a test verdict.

Complete evidence remains outside `target/` in the integration worktree:

- `.scratch/native-pool-frozen-result-long/report.json`
- `.scratch/native-pool-frozen-result-long/Pool/native.json` (all 47 case records)
- `.scratch/native-pool-frozen-result-long/Pool/native.log` (full raw output)
- `.scratch/native-pool-frozen-result-long/Pool/native.bin`
- `.scratch/native-pool-frozen-result-long/toolchain/provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/`

The incomplete 600-second compile attempt is retained in `.scratch/native-pool-frozen-result/`.
