# 05: Verify and demonstrate on a loaded server

Status: done

## Demonstration

The feature is worth what its output shows, so the acceptance evidence is a real dump from a real
compiled binary under real load, not a unit test.

```
kiln build --node --profile native-release bench/server/serve.mjs -o /tmp/serve
/tmp/serve &                                    # native-release, wall clock, one realm
bash bench/server/drive-multi.sh 4 16 5000 18088 hello
kill -USR1 <pid>                                # while the load is running
```

`--profile native-release` matters: `kiln build` defaults to `native-dev`, and NOTES.md
(2026-09-03) records a 25%–68% measurement error from exactly that mistake.

## Verification

- `mbx test -p kiln-backend-c -p kiln-native -p kiln-native-number-ffi -p kiln-cli --offline --no-fail-fast`
- `bash scripts/check-native-number-ffi.sh`
- `bash scripts/native.sh`
- `KILN_GC_STRESS=1 KILN_KEYS_CHECK=1 bash scripts/native.sh`
- `./target/debug/kiln test conformance/node --node`
- `mbx clippy --workspace --offline`
- `cargo fmt --check`

Always `--no-fail-fast` on the multi-crate run.

## Result

Done. The loaded and idle dumps are in the PRD. The four-worker dump in full:

```
processes holding the port: 388521 388523 388524 388525
{"processes": 4, "connections": 64, "rps": 63186, "errors": 0, "wrong": 0, "p50_ms": 0.813, "p99_max_ms": 2.726}

kiln introspect pid=388521 heap bytes=84002 count=1693 since_collection=78399 survived=5603 floor=262144 epoch=45 collections=45 freed_bytes=11806929 enabled=1 collecting=0
kiln introspect pid=388521 runtime live_tasks=0 park_count=25878 busy_us=590788 elapsed_us=3099011 queue_depth=0 workers=1
kiln introspect pid=388521 connections server=17 client=0 requests_in_flight=0
kiln introspect pid=388523 heap bytes=223876 count=4479 since_collection=218697 survived=5179 floor=262144 epoch=39 collections=39 freed_bytes=10232231 enabled=1 collecting=0
kiln introspect pid=388523 runtime live_tasks=0 park_count=23276 busy_us=481640 elapsed_us=3098982 queue_depth=0 workers=1
kiln introspect pid=388523 connections server=15 client=0 requests_in_flight=0
kiln introspect pid=388524 heap bytes=117112 count=2358 since_collection=112569 survived=4543 floor=262144 epoch=34 collections=34 freed_bytes=8920409 enabled=1 collecting=0
kiln introspect pid=388524 runtime live_tasks=0 park_count=20526 busy_us=447300 elapsed_us=3098105 queue_depth=0 workers=1
kiln introspect pid=388524 connections server=12 client=0 requests_in_flight=0
kiln introspect pid=388525 heap bytes=246242 count=4918 since_collection=240003 survived=6239 floor=262144 epoch=56 collections=56 freed_bytes=14693787 enabled=1 collecting=0
kiln introspect pid=388525 runtime live_tasks=0 park_count=31586 busy_us=669094 elapsed_us=3098988 queue_depth=0 workers=1
kiln introspect pid=388525 connections server=20 client=0 requests_in_flight=0
```

Sorted by pid, because the four workers interleave their writes; one `write(2)` per line is what
makes that sortable at all rather than a character salad. Each worker's `survived` is about a
quarter of the single-process figure, which is the same live set divided four ways.

## Verification results

| Check | Result |
| --- | --- |
| `mbx test -p kiln-backend-c -p kiln-native -p kiln-native-number-ffi -p kiln-cli --offline --no-fail-fast` | pass |
| `bash scripts/check-native-number-ffi.sh` | pass (exit 0; the new export is in the packaged symbol table) |
| `bash scripts/native.sh` | 76 matched, 2 refused, 0 diverged — unchanged |
| `KILN_GC_STRESS=1 KILN_KEYS_CHECK=1 bash scripts/native.sh` | 76 matched, 2 refused, 0 diverged — unchanged |
| `./target/debug/kiln test conformance/node --node` | 35/35 pass |
| `mbx clippy --workspace --offline` | no new warnings |
| `cargo fmt --check` | clean |

## One trap worth recording

The first loaded dump came back with 352 NUL bytes in front of it. Not a formatter bug: the run
truncated the log with `: > serve.err` while the server still held the fd, so the next `write`
landed at the old offset and the gap read back as NULs. A `write(2)`-based diagnostic writes at
the file offset like anything else; truncate the file the process is writing to and you get a hole,
not a fresh start.
