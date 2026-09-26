# 07. Ownership inventory and the duplicate count

Status: ready-for-agent

ADR 0060 calls for an ownership inventory; none exists yet. Build it, generated where possible:

- One row per built-in member (from `kiln-builtin-surface`) and per provider export.
- Its owner kind (ADR 0060 kinds 1 to 7) and the file that owns it.
- Every engine implementation that exists for it: interpreter `Native` variant, C method code,
  TypeScript definition. These are *found* by scanning, not declared, so a duplicate nobody listed
  still shows up.
- For kind 5 (temporary duplicate): reason, differential authority, deletion condition.

The inventory reports the **duplicate count** (members with more than one implementation) per
family and in total, beside the reported counts in the PRD. The baseline is checked in; the gate
refuses an increase. Lowering it is a baseline diff committed with the change.

The existing native surface inventory (ADR 0063, `.scratch/native-real-libraries/`) is the
starting point; extend it rather than building a second one.

## Acceptance

- `mise run gate` fails when a change adds a second implementation of a member without a kind 5 row.
- The PRD's success table is filled in with exact numbers.
