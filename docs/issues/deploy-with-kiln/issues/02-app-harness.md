# 02. The app harness

Status: ready-for-agent

A new corpus, `conformance/apps/<app>/`, each directory holding:

- the app's source, unmodified from what a user would write, plus `package.json` and its lockfile;
- a `manifest.toml` naming the entry point, the services it needs, and the request script;
- the request script: a fixed sequence of requests (HTTP, WebSocket frames, or CLI invocations)
  and the expected normalised responses, recorded from Node.

A runner, `scripts/apps.sh` (or a `kiln test` runner if architecture-consolidation issue 17 has
landed), which for each app:

1. installs dependencies from the lockfile into a cache outside `/tmp` (see `CLAUDE.md`);
2. starts the services it declares (Postgres, Redis, MySQL, MongoDB, a local TLS endpoint) with
   `docker compose`, locally;
3. runs the app under Node (the oracle), the kiln interpreter, and a `kiln build` native binary;
4. drives each with the request script and compares the normalised responses.

Normalisation strips only what differs by host and time: the `Date` header, generated ids the app
prints, and ports. Everything else is compared byte for byte, as `conformance/thirdparty` does.

The server corpus's rule carries over: requests are sequential unless the app's point is
concurrency, so a recorded output never depends on scheduling.

Outcomes use the existing vocabulary: **matched**, **refused** (names the member; backlog, does not
fail), **diverged** (answered differently; fails). A missing or short corpus is an error, never a
clean run of zero (`CLAUDE.md`, "A vanished corpus reads as a clean run").

## Acceptance

- `scripts/apps.sh express-rest` reports interpreter and native outcomes against Node.
- The results envelope has the same shape as the gate's step envelopes under `target/gate-logs/`.
- Adding an app is a directory and a manifest, with no runner change.
