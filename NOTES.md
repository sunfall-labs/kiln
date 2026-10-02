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

---

## kiln's own modules, audited: actors, shared, parallel, process (2026-10-02)

An audit of `kiln:actors`, `kiln:shared`, the parallel family and the packages around them found
more in the engines than in the modules. What landed, and what is worth knowing next time:

- **`continue` inside `try` unprotected the rest of the body, natively.** `kiln-graph` computed a
  block's exception handler by scanning code in order, so the `PopHandler` a `continue` jumps past
  ended the protected region for everything after it; the next throw escaped every `catch`. It
  surfaced as an actor realm dying on a method's first throw. Handler stacks now travel along CFG
  edges (`conformance/native/try-early-exit-handlers.mjs`).
- **A worker's ports outlived it.** A port that went to a worker in a message stayed open after
  the worker died, so its partner never saw `close`, and every actor call in flight when an actor
  crashed waited forever. Endpoints now record the worker realm holding them and close with it.
- **The interpreter evaluated modules once per import plan, not once per realm.** `import()` of a
  module the static graph already ran gave a second instance, which is why `kiln:actors` used a
  forgeable `Symbol.for` brand. One module map per realm now (`module_map.rs`).
- **A native realm asleep on a timer ignores port and worker wakes**, and a worker beside a
  pending timer may not run at all until it fires. In flight on `work/audit-tla` at the time of
  writing; every existing worker probe spawns itself, which is why none caught it.
- **Native workers from `{ eval: true }` cannot run** (no run-time compiler) -- a probe that
  swallows the worker's `error` reads as any other failure. Spawn `new URL(import.meta.url)`.
- **The daemon wedges behind a program that never ends.** A hung `kiln run` holds the daemon,
  and every later client blocks in the handshake. Use `--one-shot` for anything that might hang,
  and `kiln daemon stop` when a run stops printing.
- **kiln has no unreferenced timers**: `setTimeout().unref()` and `AbortSignal.timeout` keep a
  program alive where node exits. Not fixed; a probe that relies on node exiting early diverges.

Packages: `@kiln/concurrency` and `@kiln/effect` are gone; `@kiln/types` declares every `kiln:`
module (ADR 0068's 2026-10-02 amendment). With several agents building at once the disk hit 100%
twice and mbx's auto-gc evicted live targets: build once at the end, and clear your own
`.kiln/bin` after every corpus run.
