# Native Hash acceptance for bounded generic direct calls

This is a full 25-case native replay of the pinned Effect `Hash` test file using the unchanged upstream per-case 5-second deadline. The frozen CLI includes the bounded known-callee analysis from source commit `6d5fe097bc9bb81816f6b845a495b846dc724883`.

Result: **24 passed, 1 failed, 0 skipped**. The sole failure is the same pre-existing timeout: `Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`. This candidate does not clear the Hash blocker; it introduces no additional failed cases.

The native AOT build took 86.70 seconds and emitted 184,196,985 bytes of C / 168,310,032 bytes of application object. The case runner took 9.05 seconds total. Exact hashes, source pin, provider identity and log digests are in `receipt.json`; raw native stdout/stderr, build log, per-case report and inventory are retained beside it.

The first supervisor invocation had an incorrect frozen-CLI path and exited before any case ran. It is not part of this acceptance result.
