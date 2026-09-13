# Native ghui acceptance probe

This probe uses PR64 source commit `afd00085f5aa925bb8220a091f932b1bc21e1ac8`.
Run it from the kiln checkout with the app checkout available at
`/Users/alee/Developer/Personal/ghui-pr64`.

Build the source entry (the output path is resolved relative to the app
checkout by the native build command):

```sh
target/debug/kiln --one-shot build \
  /Users/alee/Developer/Personal/ghui-pr64/src/index.tsx \
  --node --profile native-release \
  --output .scratch/ghui-build2/ghui
```

Run the read-only mock probe in a PTY. `GHUI_MOCK_PR_COUNT=5` is deliberate:
the first frame must show `REPOS 4`, `PULL REQUESTS 5`, mock rows, and the
footer hints. Send a down-arrow to change the selected pull request, then
`q`; the terminal must be restored and the process must exit 0.

```sh
GHUI_MOCK_PR_COUNT=5 GHUI_MOCK_REPO_COUNT=4 \
GHUI_MOCK_WORKSPACE_PREFERENCES_PATH=off \
/Users/alee/Developer/Personal/ghui-pr64/.scratch/ghui-build2/ghui
```

The no-mock probe is also read-only. It reaches startup, retries the live
fetch six times, and currently exits with `kiln: uncaught TypeError: a object
is not a function`; that is the process/host capability seam owned by the
host-capability work, rather than a compiler or OpenTUI failure.

The verified mock executable was an arm64 Mach-O, 103417736 bytes, with
SHA-256 `95f9bc17792f21ed69cfea9897daaaf154d9d247f83160517abecf05b6cd1397`.
