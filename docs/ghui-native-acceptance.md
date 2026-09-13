# Native ghui acceptance probe

The app fixture is PR64 source commit `afd00085f5aa925bb8220a091f932b1bc21e1ac8`, with the local process capability adapter at `9484c3f`. The app checkout used for the latest probe is `/Users/alee/Developer/Personal/ghui-process-capability`; the compiler checkout and binary are kept separate.

Build the app entry from the kiln checkout. Use an output path in the app checkout so the command and subsequent run identify the same binary:

```sh
target/debug/kiln --one-shot build \
  /Users/alee/Developer/Personal/ghui-process-capability/src/index.tsx \
  --node --profile native-release \
  --output /Users/alee/Developer/Personal/ghui-process-capability/.scratch/ghui-build/ghui
```

The historical mock PTY probe used `GHUI_MOCK_PR_COUNT=5`, showed the repository and pull request rows, changed selection with a down arrow, restored the terminal with `q`, and exited 0. It is retained as mock renderer evidence; it is not the final live acceptance.

The current live process boundary has a smaller reproducible read-only probe at `/Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe.ts`. Build and run it with:

```sh
target/debug/kiln --one-shot build \
  /Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe.ts \
  --node --profile native-release \
  --output /Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe-clock417

/Users/alee/Developer/Personal/ghui-process-capability/.scratch/native-process-boundary-probe-clock417
```

The verified binary was built from integration `417281af`, has SHA-256 `41f697edd310b6ab5025614164161d45daeaf1778ef8941e3782b540732e995a`, and produced `AFTER_RUN 0 1084`, `AFTER_SCHEMA 1`, and `DONE` in 1.23 seconds against the real read-only `gh api graphql` command. The provider archive used by the corrected packaging path was SHA-256 `eb5a9bbfbc531985e811803a344f3269971185aea3687b6ecaf490031776e00a`.

The full live PTY session still requires final verification after native dynamic module lifecycle work lands. This document does not attribute the earlier `object is not a function` report to one subsystem: investigation also found a native static-field receiver bug and a virtual versus wall-clock timer wake bug, both fixed in the compiler/runtime integration history.
