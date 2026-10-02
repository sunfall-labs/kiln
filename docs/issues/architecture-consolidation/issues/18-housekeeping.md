# 18. Housekeeping

Status: in-progress (three of four items done 2026-10-02; `CLAUDE.md` pruning waits on 14 to 17)

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

## Comments

**2026-10-02.** Three of the four items are done:

- **ADR numbers.** The shared-runtime-contracts ADR is now 0076, not the intrinsics one: every
  existing reference to "ADR 0059" meant intrinsics, so renumbering the other moved no meaning.
  `scripts/new-adr.sh <slug> "<title>"` takes one above the highest number in the checkout and on
  origin/main, and the `adr_numbers` crate test in `kiln-cli` fails the gate on a duplicate.
- **`NOTES.md`.** Archived whole, verbatim, to `docs/notes/2026-08-29-to-2026-10-02.md`, with a
  fresh `NOTES.md` that points there. Not split by month: the log was never chronological
  (standing sections sit between dated entries), so a month split would have misfiled them.
- **`.scratch/` binaries.** Moot: the tracker moved to `docs/issues/` after `.scratch/` was
  cleared and all 634 filings were committed as deleted (`497bd9a4`), and `.scratch/` is now
  ignored wholesale.

Left open: **`CLAUDE.md`** pruning, which depends on 14 to 17 landing.
