# Hash interpreter profile, 2026-10-05

This is a diagnostic profile of the unchanged pinned Effect `Hash` entry under the interpreter. It is evidence for where the measured work landed, not a quiet performance baseline or an acceptance verdict.

## Provenance

- Source checkout: `~/Work/kiln/.scratch/effect-sandbox-integration`
- Frozen source commit used for the CLI: `22e662a6f40f1260e9763b23055fa7a17f7f4eda` (`weak-index` candidate)
- CLI SHA-256: `b56c9911d80fcd6dfacc06051dde66958c295b79e99f544920ac9d9e8a356de8`
- Input: pinned `Hash/entry.ts`; harness inventory recorded 25 cases.
- Command and exact outcome are retained in `interpreter-status.json` and `interpreter.log`.
- The run retained the original 5-second case budget. It ran under sampling and concurrent native-survey work: 24 passed, 1 timeout, 0 skipped, total child wall time 64.087 seconds. The timeout was `Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`.
- This CLI predates the final integration and does not establish the final source's outcome.

## Profile reading

The accepted sampling window contained 3,703 samples for the failing case, between its preceding case marker at 26.93 seconds and the timeout record at 64.03 seconds. Top sampled leaves were `Vm::run_bounded` (641, 17.3%), `PropertyKeyRef::matches_slot` (243, 6.6%), `WellKnownSymbol::from_property_key` (182, 4.9%), `WeakCensus<ObjectData>::register` (136, 3.7%), `Value::drop` (124), `Value::clone` (122), `symbol::is_reserved` (116, 3.1%), `StringUnits::eq` (105), `Intrinsics::overridden_key` (101), and census pruning (95). `matches_slot` was present in 545/3,703 inclusive samples and `ObjectData::lookup_key` in 366/3,703.

The profile supports a narrow hot-path candidate: `matches_slot` recalculates symbol and internal-name classification for each candidate slot. For UTF-8 names this repeats well-known-symbol lookup and reserved-slot classification, including a scan of the internal-slot table. The source change classifies a lookup key once and reuses that result across each own-property scan and the full prototype walk. The rule itself is unchanged: `"@@"` strings are ordinary names, symbol keys retain symbol identity, reserved internal names remain internal, and UTF-16 strings continue to use exact code-unit equality.

Sampling overlap affects costs and elapsed time. No speedup is claimed until a same-workload before/after comparison can run in an authorized quiet window.

## Candidate validation

On the candidate working tree based on `22e662a6f40f1260e9763b23055fa7a17f7f4eda`, the focused regression passed (1 passed, 0 failed), followed by the interpreter crate (273 passed, 0 failed, 1 ignored; doc tests 0/0). Both used the pinned `mbx` wrapper, one build worker, one Rayon worker, and `--test-threads=1`. Full command output is retained in `focused-test.log` and `interpreter-crate-test.log`. This validates interpreter behavior only; it does not re-run the upstream Hash corpus or claim a speedup.

## Retained files and hashes

The compressed profile is `profile.json.gz`; `profile.json.syms.json` is its symbol sidecar; `markers.jsonl` retains the case boundaries; `interpreter-status.json` records the command/result; `interpreter.log` retains the full harness stream.

SHA-256:

- `interpreter-status.json`: `6575079ec5798f1490841231409356d838cc41d06b1169d643acc4ce8a73aeb3`
- `interpreter.log`: `8e87ad85ef42fff33c07b8c6323afaf9e6c73d70c5edbe046761997c98e3b492`
- `markers.jsonl`: `bc15064271c43975ad120e166e130cdc7d3f5832253f5fe19b26c1415d19962c`
- `profile.json.gz`: `af7e045f3315a1b06fc03313cf5191be9f30f1d75811d3c9658507632b8799ef`
- `profile.json.syms.json`: `0aaa0c249e458e781689a2aa911b971a2849a9a58c6876216facae6777d37f31`
- `focused-test.log`: `333e5e1278cc67d04791f842e23eeaf6fa7d449965b8f7e007b0493cd8e9d951`
- `interpreter-crate-test.log`: `2b612b8921a78d5957566757de1349d2a04978a0cd7081e1fd53fd147c977789`
- `candidate.patch` (source diff against the recorded base): `432dc5a38e6384ea4378afa7aa8b4df77d1df9fba8318056ed5cfc5b4c70a22e`
