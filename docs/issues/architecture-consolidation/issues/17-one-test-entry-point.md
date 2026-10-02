# 17. One test entry point

Status: ready-for-agent

`CLAUDE.md` has a table of which runner and flags each corpus needs, and says the wrong one
"produces confident, wrong numbers". The manifests already carry some of this
(`requires_host = "node"`). Finish it: `kiln test <dir>` reads each corpus manifest and applies the
runner, host and flags it declares, including delegating to the native, differential, thirdparty
and Test262 runners.

## Acceptance

- `kiln test conformance/<any>` gives the same result as the runner in `CLAUDE.md`'s table.
- The table is deleted from `CLAUDE.md`, replaced by one line.
