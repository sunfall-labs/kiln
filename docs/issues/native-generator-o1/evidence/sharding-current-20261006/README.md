# Current combined-source Sharding replay

This full unchanged Sharding replay used the combined frozen CLI and pinned ABI 82 provider. It preserved all 66 case IDs and original 5,000 ms test deadlines. The run completed 61/66 with five timeouts; exact case IDs and the comparison with the retained 61/66 candidate are in `receipt.json`. Every case outcome matches the retained 61/66 run exactly; no failure cleared and none was added.

The one-file cold AOT build completed in 1,218.648 s (38.179 s emission, 1,174.686 s compile, 0.340 s link); native execution took 52.171 s. It produced 346,087,591 bytes of generated C and a 212,889,256-byte binary. The verified compressed executable is retained outside Git at the replay workspace and is identified by receipt SHA-256. These durations are not a quiet performance result: bounded native compilation overlapped the replay.

The complete 66-case native stream, build log, runner envelope, pinned provider manifest, job specification/status, and retained baseline report/logs are included here. The execution used `--engine native --path cluster/Sharding --native-profile native-release --jobs 1 --runtime-jobs 1 --kiln-memory-mb 4096 --timeout 1800 --compress-native-artifacts --cache-gc`; no GC stress or realm fanout was enabled.
