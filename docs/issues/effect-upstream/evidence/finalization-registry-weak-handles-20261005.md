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
  `f510cacb22f8c8fcb989cab8124607cd26295c8a3b0d0c850407820c992d394c`.
- A native Atom attempt using CLI `0a0b638392a50a2aa4c1c2e1c671e8a09746fd577260b162d20d55087078a771`
  reached the 600-second outer compilation deadline before producing an executable or registering
  any cases. Its report is explicitly incomplete and is not a native test verdict.

The two Effect reports and raw streams remain in the ignored workspace under
`.scratch/finalization-atom-interpreter-validation/` and
`.scratch/temporal-now-atom-native-validation/`.
