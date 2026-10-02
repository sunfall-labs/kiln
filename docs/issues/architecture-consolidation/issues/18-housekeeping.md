# 18. Housekeeping

Status: ready-for-agent

- **ADR numbers.** Two ADRs are numbered 0059
  (`0059-intrinsics-are-ordinary-realm-objects.md`,
  `0059-shared-runtime-contracts-own-cross-cutting-metadata.md`). Renumber the later one, update
  references, and add `scripts/new-adr <slug>` (or an xtask) that takes the next free number and
  writes the template, so parallel sessions stop colliding.
- **`NOTES.md`** is 5,981 lines. Archive it by month under `docs/notes/`, keep the current month in
  `NOTES.md`.
- **`.scratch/` binaries.** `.scratch/<feature>/` is tracked on purpose, but the root holds many
  untracked compiled binaries (`*_bin`, `*_release`, `*_prof`). Add ignore patterns for build
  outputs there without ignoring the tracked filings.
- **`CLAUDE.md`.** As each of 14 to 17 lands, delete the warning it made unnecessary. Its length is
  the progress measure.
