# 05. Refusals name the package and the import chain

Status: ready-for-agent

When `kiln build` refuses inside `node_modules`, the user did not write that code and does not know
why their app reached it. A refusal from a dependency reports:

- the member refused and why (the existing message);
- the package and version, and the file and line inside it;
- the import chain from the user's entry point to that file;
- whether the refusal is tracked (a link to the issue or inventory row) and whether a known
  workaround exists (a different package version, an option that avoids the path).

`kiln check <entry>` reports **every** refusal the build would hit in one pass rather than stopping
at the first, grouped by package. That report is the answer to "will my app run on kiln?", and the
first thing a user will run.

## Acceptance

- A refusal inside a dependency prints package@version, file:line and the import chain.
- `kiln check` on the stretch apps (issue 15) lists all their refusals in one run.
