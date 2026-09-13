# Native ghui acceptance probe

The live acceptance passed on app source fixture `afd00085f5aa925bb8220a091f932b1bc21e1ac8` with
the local process capability adapter at `9484c3f`. The compiler production revision was `66d07dd9`
and the integration checkout used for the build was `01aaaca5`. The exact
verified binary was `/Users/alee/Developer/Personal/ghui-process-capability/.scratch/ghui-live/ghui-66d07dd9`
(SHA-256 `e112a489242695ad94a14bb6b16f1f47e96a9243792ebe88a8399fe8d6c7fefc`, 112,840,824 bytes,
mtime 2026-09-13 05:00:24). The build log is
`/Users/alee/Developer/Personal/kiln-ghui-integration/.scratch/ghui-66d07dd9-build.log` and the
terminal transcript is
`/Users/alee/Developer/Personal/ghui-process-capability/.scratch/ghui-live/66d07dd9-live.log`.

Build the app entry from the kiln checkout. Use an output path in the app checkout so the command and subsequent run identify the same binary:

```sh
target/debug/kiln --one-shot build \
  /Users/alee/Developer/Personal/ghui-process-capability/src/index.tsx \
  --node --profile native-release \
  --output /Users/alee/Developer/Personal/ghui-process-capability/.scratch/ghui-build/ghui
```

The live read-only session used the real GitHub account `candrewlee14` and observed 16 repositories,
50+ pull requests, and 15 issues. It selected PR #3, entered its details, traversed the markdown
details with a down-scroll, returned with Escape, restored the terminal with `q`, and exited 0.
No GitHub mutation was performed.

The earlier mock PTY probe remains historical renderer evidence. It is not used as evidence for this
live result.

The current live process boundary has a smaller reproducible read-only probe at `/Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe.ts`. Build and run it with:

```sh
target/debug/kiln --one-shot build \
  /Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe.ts \
  --node --profile native-release \
  --output /Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe-clock417

/Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe-clock417
```

The verified binary was built from integration `417281af`, has SHA-256 `41f697edd310b6ab5025614164161d45daeaf1778ef8941e3782b540732e995a`, and produced `AFTER_RUN 0 1084`, `AFTER_SCHEMA 1`, and `DONE` in 1.23 seconds against the real read-only `gh api graphql` command. The provider archive used by the corrected packaging path was SHA-256 `eb5a9bbfbc531985e811803a344f3269971185aea3687b6ecaf490031776e00a`.

The acceptance exercises the timer payload/keep-alive fix through the same asynchronous process path.
The corresponding release regression was run in both the normal configuration and with GC stress;
the process capability and timer liveness fixtures remain the durable checks for that behavior.

There was one transient footer notice while entering PR details: `Timed out after 15000ms`. This is
the ghui command runner's blanket 15-second command budget, not a proof of a runtime failure. The
transcript has no command IDs or timestamps, so it cannot identify whether the selected detail query
or a neighbor prefetch crossed the budget. Full PR details rendered afterward and navigation, scroll,
back, terminal restoration, and exit all succeeded. A local follow-up records the app-side work to
add per-operation telemetry and distinguish foreground detail requests from silent prefetches.

The repository-wide gate was still running at the time of this acceptance record (`558b05c6`). Native
parity had an independently investigated callable-name divergence; native stress and push checks were
pending. This document records the ghui acceptance only and does not claim the overall gate complete.
