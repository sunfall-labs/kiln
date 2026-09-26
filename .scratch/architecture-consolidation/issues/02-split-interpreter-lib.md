# 02. Split `kiln-interpreter/src/lib.rs` into modules

Status: ready-for-agent

`crates/kiln-interpreter/src/lib.rs` is 49,652 lines. It is the slowest file in the workspace to
rebuild, the worst to edit, and the one most sessions collide in.

## Approach

A **pure move**: items change file and visibility (`pub(crate)`), nothing else. Suggested shape,
to be adjusted to what the file actually contains:

- `vm/` (dispatch loop, frames, calls, exceptions, generators and async)
- `object/` (`ObjectData`, properties, shapes, descriptors, prototype walk)
- `realm/` (realm construction, intrinsics table, capabilities)
- `intrinsics/<family>.rs` (the `Native` arms per family; these shrink as 10 and 11 land)
- `modules/` (module records, loaders, CommonJS)
- `host/` (host-op adapters)

One slice per commit, each landed as soon as it is green, so no long-lived branch conflicts with
other sessions. Work in a worktree you own (`CLAUDE.md`).

## Acceptance

- No source file in `kiln-interpreter/src` is over 5,000 lines (tests included; see 03).
- `mbx test -p kiln-interpreter --offline` reports the same pass count before and after each slice.
- `git diff --stat` for each slice shows lines moved rather than changed. Ideally a
  `git diff --color-moved` shows no non-move lines other than `mod`/`use` and visibility.
