# Issue tracker: Local Markdown

Issues and PRDs for this repository live as Markdown files in `docs/issues/`.

`.scratch/` is untracked local scratch paper and may be emptied at any time; nothing in it is
committed. A reference to `.scratch/<feature>` in an ADR or in `NOTES.md` -- both are records that
are never edited in place -- means `docs/issues/<feature>`.

## Conventions

- One feature per directory: `docs/issues/<feature-slug>/`
- The PRD is `docs/issues/<feature-slug>/PRD.md`
- Implementation issues are `docs/issues/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`
- Triage state is recorded as a `Status:` line near the top of each issue file
- Comments and conversation history append to the bottom of the file under a `## Comments` heading
- Only Markdown and small probe sources (`.md .mjs .cjs .mts .ts .js .py .sh .patch .rs .toml`)
  are committed under `docs/issues/`; `docs/issues/.gitignore` keeps logs, JSON dumps, tarballs,
  profiles and other run evidence local. An issue may cite evidence files that are not in the
  repository; summarize the numbers in the issue text.
- Throwaway files that nothing cites belong in `.scratch/`.

## Deleting a filing is deliberate

`scripts/check-issue-deletions.sh` refuses a commit or a push that deletes a file under
`docs/issues/` without moving it. It runs from `.githooks/pre-commit` and `.githooks/pre-push`
(`mise run hooks` installs both). A closed issue keeps its file with `Status: closed`; it is not
removed. When removing filings is genuinely intended, say so for that one command:

```sh
KILN_ALLOW_ISSUE_DELETION=1 git commit ...
```

The crate test `issue_tracker` in `kiln-cli` is the backstop for a hook that never ran: it fails
the gate if `docs/issues/` is missing or holds fewer filings than `docs/issues/.floor` records.

## When a skill says "publish to the issue tracker"

Create a new file under `docs/issues/<feature-slug>/`, creating the directory if needed.

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or issue number directly.
