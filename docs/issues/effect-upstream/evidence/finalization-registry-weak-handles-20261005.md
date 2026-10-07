# FinalizationRegistry weak handles — 2026-10-05

`FinalizationRegistry.register` recorded ordinary targets and unregister tokens as
`WeakCellValue::Other(Value)`. That stored a strong `Rc` in the registry while the collector
deliberately omitted target/token links from `object_children`, because those links are weak in
JavaScript. The resulting unaccounted owner made a registered object look externally rooted.
Effect's `Atom.family` registers every cached atom, so this prevented its old atom from being
collected.

`WeakCellValue` now delegates identity storage and upgrade to `WeakCollectionKey`, whose variants
hold weak references for heap-backed values and copy identity for non-collectable builtins. The
existing strong `FinalizationCell.held` edge is unchanged. `register` validates targets and tokens
before constructing these handles; `object_children` continues to trace only holdings.

Validation:

- `finalization_registry_targets_and_tokens_are_weak_but_holdings_are_strong`: passed. It covers
  unregister identity and repeated unregister, an unrooted self-cycling target, weak token
  lifetime, a holding object that keeps a target alive until its edge is removed, and an external
  target root.
- `mbx test -p kiln-interpreter -- --test-threads=1`: 260 passed, 0 failed, 1 ignored.
- Pinned Effect `reactivity/Atom`, direct TypeScript, interpreter: 125/125 passed, with 125 Node /
  Vitest cases mapped and no inventory, mode, or outcome mismatches. CLI SHA-256:
  `403d9b682677ff94a9c149fd92ac8dfc8a96467d5369f1cefb63f4b538bd7117`. Report SHA-256:
  `f510cacb22f8c8fcb989cab8124607cd26295c8a3b0d0c850407820c992d394c`. Source pin:
  `67ba4e46a11ccda0b6761578bfd22c04ae00167d`; source file SHA-256:
  `7d82b7fab16650ebdeb7bee6e7841effb7f0be70e5970360119584948b0b507b`. Runner commit and digest:
  `4f39b9dca8fa5ff1ff4980180f2e5db15fdd20ae` and
  `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`. The run used one job,
  one runtime worker, an 8 GiB child limit, and a 600-second outer runner allowance; this outer
  allowance did not alter upstream case deadlines.
- A native Atom attempt using CLI `0a0b638392a50a2aa4c1c2e1c671e8a09746fd577260b162d20d55087078a771`
  reached the 600-second outer compilation deadline before producing an executable or registering
  any cases. Its report is explicitly incomplete and is not a native test verdict.

The complete interpreter receipt (full report, per-file report, and raw log) is archived at
`atom-interpreter-20261005.tar.zst`, SHA-256
`0ea96d006a281f8a3fe436caf0a4fe208c4c0a26ed2c439c1c48a54219581682`. It contains the full Atom interpreter receipt plus the focused native GC logs. It can be inspected with
`tar --zstd -tf` and restored with `tar --zstd -xf`. The selected native GC probes (`weak-references-basic`, `weak-references-job-rooting`, and the
three saved-generator GC probes) matched in both ordinary and `KILN_GC_STRESS=1` runs: 5/5
matched, 0 refused, 0 diverged, and no stress skips. Raw logs are included in the archive. Both
runs used the same verified CLI SHA `403d9b682677ff94a9c149fd92ac8dfc8a96467d5369f1cefb63f4b538bd7117`.

The exact CLI snapshot is retained outside the
archive at `.scratch/finalization-atom-interpreter-validation/toolchain/kiln-403d9b682677ff94a9c149fd92ac8dfc8a96467d5369f1cefb63f4b538bd7117`; its SHA-256 is recorded above. The source workspace and generated report are
also retained locally under `.scratch/finalization-atom-interpreter-validation/`. The incomplete
native attempt remains local under `.scratch/temporal-now-atom-native-validation/` and is not
presented as a result.
