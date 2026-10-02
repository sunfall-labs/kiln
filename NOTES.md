# NOTES

Working notes between the agents and people building kiln. Append rather than rewrite; if you
disagree with something here, say so under your own heading instead of editing someone else's.

Durable architectural decisions belong in `docs/adr/`, not here, and issues belong in
`docs/issues/`. This file is for the things that are true this week: who is in which files, what
just broke, what to watch out for.

**Earlier notes are archived, verbatim, in `docs/notes/`.** The first log ran from 2026-08-29 to
2026-10-02 and reached 6,027 lines: [`docs/notes/2026-08-29-to-2026-10-02.md`](docs/notes/2026-08-29-to-2026-10-02.md).
A comment elsewhere in the repository that says "see NOTES.md" means that file unless it names a
date after 2026-10-02. It was not split by month because it was never chronological: standing
sections (conventions, gotchas, measurement discipline) sit between dated entries, and a mechanical
split would have filed them under whichever month they happened to follow. When this file grows
past a few thousand lines, archive it the same way.

---

## The log has a gap: 2026-09-19 to 2026-10-01 (2026-10-02)

The archived log has nothing dated between 2026-09-19 and 2026-10-01. The work did not stop; its
record moved into the issue tracker and the ADRs. For that period read `docs/issues/` (the
`npm-compat`, `deploy-with-kiln`, `full-runtime`, `realms-and-concurrency`, `ghui-opentui` and
`architecture-consolidation` features above all) and ADRs 0060 to 0076. Commit messages from that
range are the third source: `git log --since=2026-09-19 --until=2026-10-02`.

---

## The issue tracker was deleted and restored; it lives in `docs/issues/` now (2026-10-02)

Commit `497bd9a4` (a GUI change) also deleted all 634 tracked filings under `.scratch/`, 284,000
lines, without a word in its message: they had been cleared on disk and `git add -A` recorded it.
They are restored byte for byte from `497bd9a4^` into `docs/issues/`, where the directory's name
no longer says "disposable". `.scratch/` is now wholly ignored. Deleting a filing is refused by
`scripts/check-issue-deletions.sh` from the pre-commit and pre-push hooks unless
`KILN_ALLOW_ISSUE_DELETION=1` is set, and the `issue_tracker` crate test fails the gate if the
tracker shrinks below `docs/issues/.floor`. Details in `docs/agents/issue-tracker.md`.

The same commit is a lesson about `git add -A` in a checkout other sessions write to: read the
`git status` summary line before committing, and a deletion count in the hundreds is never part of
a GUI change.

---

## "kiln never invokes the `listen` callback" was the output path, not the engine (2026-10-02)

The 2026-09-18 entry in the archived log says `server.listen(port, host, cb)` never fires its
callback. It always did: before the first request, on both engines, in all four `listen` forms
(`conformance/server/s09-listen-callback.mjs` pins the event, callback and request order). What
lost "listening" was `kiln run`: `Run` and `NativeRun` answered with one summary after the program
exited and carried its stdout and stderr in it, so a program that never exits -- any server --
printed nothing until it was killed. Protocol 38 adds `stream_output`: the daemon sends
`Response::Output` frames ahead of the summary, over the socket or the `--one-shot` channel, and the
CLI writes and flushes them as they arrive. A corpus run captures in-process and cannot see the
difference; the `kiln-cli` one-shot test `a_servers_listening_line_reaches_the_client_before_the_server_exits`
is the one that fails without streaming.
