# Mixed-tier Sharding symbolized diagnostic profile (2026-10-05)

This is a diagnostic CPU profile of the retained mixed-tier Sharding AOT binary. It preserves the original per-case 5-second deadlines and is not an acceptance/performance result: samply instrumentation changed the outcome from the prior quiet replay's 61/66 to 60/66.

The archived quiet report identifies candidate commit `7461336fd18ea428ac66d3b8587622709f61f35c`, CLI SHA-256 `8c2a4ada35219f76444d21f28c8aa4041d8720a416e09ad561a5b9dc0f4254f2`, and adapter harness SHA-256 `778fceda42fdaa5597825fa4d99bfd4d47044883efe48cf0a1332e3127a53913`. This predates the hook/deadline adapter correction in `827129f2` (current harness `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`) and the combined Array/constructor fixes. Original test-body deadlines were retained, but this evidence does not certify the corrected adapter or final combined source.

The symbols-only relink used `KILN_NATIVE_KEEP_SYMBOLS=1`; it reused generated C and all app/prelude/runtime/launcher/provider objects, rebuilt only the final executable link, and retained the same source and compiler/optimization policy. The profiled executable SHA-256 is `631280990eb04580de181d39244bf08752687e09dbb84bcacbfd7e33693f3245`, ELF Build ID `9aaf7e609e8cddefecb302bb3d0583d0f990ca5d`. The quiet comparison executable SHA-256 is `b4bd44aaebbfb84a3a9fad324555f23843c63e849c410d39fcd140443c9f8877`.

Inputs: pinned upstream Effect source commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`; direct TypeScript file `workspace/files/cluster/Sharding/entry.ts` SHA-256 `68997ac20b91925ae52e3a3ab097696152b426b0dcfdef0d86869ab462092702`; upstream Sharding implementation SHA-256 `057a856588d4d91e87ddf1659fbf0bf56a14e5a96332ee505cba95190466cdc0`; runner `scripts/effect-upstream.py` SHA-256 `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`; provider manifest SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`, archive SHA-256 `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d` (ABI 82). The quiet comparison report preserves the registered 66-case inventory, exact pass/fail outcomes, all harness file digests, and quiet native build metadata. The profiled run’s exact inventory/outcomes are in the marker/log files. Upstream source `cluster/Sharding` SHA-256 from the runner report is `0040998907b12af77ac53b533b0ddf19ca9f7bac552502d524d256a72696fed0`.

`analysis.json` records marker-aligned sample counts, named native symbols, selected generated-function source mapping, and the limits of the evidence. Cause formatting functions are present in passing as well as failing intervals, so these samples cannot be attributed solely to timeout reporting. The native dispatch leaf is small compared with the complete call stack, while the large Sharding function 77 is O0 and accounts for only about 4% of samples in each timeout interval. No production change follows from this profile alone.

Archived evidence (all compressed except the already-gzipped profile):

- `analysis.json`: 21378 bytes, SHA-256 `6fd2742bdd6fd3b6fdc8b9011df42c1d516862a127b338e1f18f63ffefa193ef`
- `marker-arrivals.jsonl.gz`: 3382 bytes, SHA-256 `228b4579142887763361d2e9108fa7668517d57e50555b2368015ee263f7743d`
- `marker-run-receipt.json.gz`: 399 bytes, SHA-256 `3c5400dba654807ce64bc2273c2918428e13c0c4bbeb04b0e6bad517cf114e41`
- `marker-run.log.gz`: 3793 bytes, SHA-256 `8ee7e1f0aa0238404d55144d5549444916e9a9beb18ecaba1613a49ab801f51c`
- `quiet-report.json.gz`: 6401 bytes, SHA-256 `59538e235b6730933e22be57895398d3c105b8e8fdbeaa35eaed3fa3b24f7dd9`
- `profile.json.gz`: 653126 bytes, SHA-256 `2945b127ea06c7948346a696dc181df232992b1c3566de6d27486aa2118cb2fa`
- `profile.json.syms.json.gz`: 74917 bytes, SHA-256 `0fc96e6bff53713b282ec93f7ee26f90d7c61dd019fa2313f99a76f330a1262b`
