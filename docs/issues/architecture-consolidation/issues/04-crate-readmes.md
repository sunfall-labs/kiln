# 04. A README for every crate

Status: ready-for-agent

Agents decide where code goes from whatever they read first; today that is a 6k-line `NOTES.md`
and a `CLAUDE.md` that is mostly warnings. Each crate gets a `README.md` of one screen:

- **Layer** (ADR 0066) and what the crate owns, in two sentences.
- **Invariants** a change must keep.
- **Does not belong here**: the things people have wrongly put here, and where they go instead.
- **Tests**: the narrowest command that exercises the crate.

The crate's `lib.rs` `//!` doc stays the API description; the README is the orientation.

## Acceptance

- Every directory under `crates/` has a `README.md` with the four headings.
- The layer each README states matches ADR 0066's table.
