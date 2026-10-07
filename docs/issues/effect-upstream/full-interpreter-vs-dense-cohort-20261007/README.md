# Full interpreter failures versus the matched 562-case replay

This note answers one narrow question: did the older 322-file interpreter report contain
failures outside the seven files later replayed in the dense-collector comparison? It did not.
The full report recorded 10,934 passes, 40 failures, and 45 skips across 322 complete files.
The later matched published-control report replayed 562 cases in seven files and recorded 537
passes and 25 failures. All 40 failures from the older report belong to those seven files; 15 of
those older failures passed in the later replay. The remaining 315 files were **not** rerun in
that replay, so this comparison says nothing about their current status.

The source reports use the same pinned upstream revision (`67ba4e46a11ccda0b6761578bfd22c04ae00167`),
runner digest, harness digests, Node/Vitest oracle digest, and exact mapped case inventory. The
full report used CLI `f68949f7e9f66848e5e6294fa972a06be7f42378b1cbd1ccecf35bc82f00348a`; the later
seven-file control used CLI `b0ac1791e1ab0a26e51711dbaa8bd833aabbb8c016d6e7921d6244faa07639fb`.
The CLIs differ, which is why only case IDs and report outcomes are compared here; this is not a
matched performance or whole-suite current-status claim.

`audit.json` contains the complete 562-case ID set, both failure sets, the 15 old-only failures,
input hashes, toolchain/oracle identities, and the empty outside-cohort set. `compare-reports.py`
reproduces it from the archived full report and compressed matched control report:

```sh
python3 compare-reports.py /path/to/full/report.json /path/to/matched-published-control/report.json.gz \
  --output audit.json
```

The source reports used to produce this record were:

- Full: `.scratch/effect-recovery-combined-20261006/interpreter-full/report.json`
- Seven-file control: `.scratch/effect-runtime-followup/docs/issues/effect-upstream/dense-collector-matched-cohort-20261007/matched-published-control/report.json.gz`

Their SHA-256 digests, including both compressed-file and uncompressed control-report digests,
are recorded in `audit.json`. No test was run to produce this audit.
