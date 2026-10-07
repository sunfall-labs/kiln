# Complete ClusterWorkflowEngine interpreter failure evidence

The original pinned file now records all 47 registered cases: 36 pass, 11 fail
at their original 5,000 ms body deadlines, no skips, missing cases or runner
errors. All 47 map exactly to the official Node/Vitest oracle; 11 outcome
mismatches remain. The process and runner exit 1 as expected. This is negative
correctness evidence, not a completed goal or a performance score.

The old deadline adapter stopped after 30 records with 17 missing cases on the
same frozen clock CLI. The corrected adapter bounds finished-hook cleanup and
continues earlier cleanup callbacks, allowing the complete summary. Eight of
those formerly missing records pass, while nine expose further original-budget
timeouts. This does not establish whether those timeouts are independent or
affected by earlier timed-out operations.

The frozen CLI is `e7a06733713ec8acfb7e3cc981ad78b1d881befa57b6d4ab455c3ed33f5139a7`,
from source `faff5879`, before the newer using-context and Array changes. Harness
SHA is `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`,
now committed in `827129f2`. The report records the runner's start-state commit
and every input digest; documentation commits during execution changed no
executable adapter code. Source pin remains
`67ba4e46a11ccda0b6761578bfd22c04ae00167d`. Complete raw records are archived.

The run used one runtime worker, wall-clock scheduling, an 8 GiB process cap,
and a 1,800-second outer file allowance. That allowance changes no test deadline.
Other agent work may have overlapped, so the 184.638-second file duration is
diagnostic only. The combined final source must rerun this file and all 322 files.

```sh
KILN_JOBS=1 python scripts/effect-upstream.py --workspace <owned-workspace> \
  --out <output> --engine interpreter --path cluster/ClusterWorkflowEngine \
  --kiln <frozen-kiln> --provider-manifest <provider-manifest> \
  --jobs 1 --runtime-jobs 1 --timeout 1800
```
