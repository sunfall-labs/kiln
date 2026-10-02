# 12. The compatibility page

Status: ready-for-agent
Blocked by: 02, 11

A generated `docs/compatibility.md`, and a short table in `README.md`, answering "does my stack
run?":

| app | packages | interpreter | native | memory vs Node | startup | throughput vs Node / Bun |
| --- | --- | --- | --- | --- | --- | --- |

plus, for each app that does not yet pass, its top refusals with links to the issues tracking them.
Generated from the harness and benchmark results, never hand-edited, with the commit and date it
was measured at.

Publish it as soon as M1 passes, and update it every time the burn-down (14) moves a number. The
table is the product claim; it should always be current and reproducible.

## Acceptance

- `scripts/apps.sh --report` (or equivalent) regenerates the page.
- `README.md`'s performance section links to it and stops leading with hello-world.
