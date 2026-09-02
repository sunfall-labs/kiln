# 05 — Verification, measurement, ADR and NOTES entry

Status: done
Depends on: 04

## What

- Re-run everything in the PRD's baseline table on the same worktree in the same session and
  record the numbers with commit hash and binary mtime.
- `KILN_GC_STRESS=1 KILN_KEYS_CHECK=1 scripts/native.sh` (the env reaches the compiled probe
  because `kiln run --native` execs it).
- `cargo clippy --workspace --offline` (mbx absent, see PRD) and `cargo fmt --check`.
- Perf: `.scratch/perf/app-props.mjs`, `--profile native-release --node`, mixed driver, 4 × 16
  × 5 s, three passes, before and after.
- ADR: run `ls docs/adr/` immediately before committing to pick the number.
- NOTES.md: a dated entry in the log voice; nothing in CLAUDE.md.

## Comments
