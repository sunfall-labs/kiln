# Namespace refresh milestone: real-clock checks

Frozen CLI SHA-256: `8ac442d8270d0f76ea7933acef5c9812b24f1d9a1f537fbd98f25e7e43515867`.
Production source: `79a05698`; source receipt includes the combined Array, constructor, tier and host-clock fixes.
Pinned upstream source: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
Corrected deadline harness, real host wall clock, unchanged per-case 5-second budgets, one worker, 8-GiB cap.

Channel: all 48 registered/recorded cases pass and map exactly to Node/Vitest. Its original two-file process was interrupted during Hash (terminal exit 130), so its report deliberately retains one pending file. Only the completed Channel result is evidence; the interrupted Hash output is excluded.

Hash: a separate process completed (exit 1), all 25 cases recorded/mapped, 15 pass and 10 fail with `TEST_TIMEOUT:5000ms`; no outer file timeout, mapping or inventory errors. This is a remaining performance failure, not final acceptance.

These checks supersede earlier namespace checks using CLI `7584f62c`, whose interpreter still answered a synthetic 2020 epoch even with `--clock wall`. The clock-contract evidence separately confirms the new CLI has a real epoch and the expected deadline outcomes. No full-suite completion is claimed here.
