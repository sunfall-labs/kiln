# Native Hash replay after the value-call ABI change

This is a correctness replay of the original pinned `Hash` file, not a benchmark.
The call-value ABI candidate did not remove its existing hash-law timeout.

## Frozen inputs

- CLI: `kiln-a0ab428cadabf39b7e9af894ddf102855e23f6d77698c8b76e89185c32420690`, SHA-256 `a0ab428cadabf39b7e9af894ddf102855e23f6d77698c8b76e89185c32420690`.
- Candidate source recorded beside that CLI: root `3aa` plus the computed-call prototype guard patch; the CLI was supplied frozen and was not rebuilt for this replay.
- Effect source: pinned commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`; `Hash.test.ts` SHA-256 `9fbc376ce19f8de3190eb9de5e8fa5b8bd2698cf16d3617178bf401eb5f3ea0c`.
- Runner: integration `scripts/effect-upstream.py`, SHA-256 `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`, commit `3faacb4c0dbfdc43b917c27eef73cf8efb1ca11a`.
- Provider manifest: ABI 82, SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; archive SHA-256 `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`.

## Command and result

The runner selected only `Hash`, used native-release, one runner/runtime worker, a 4096 MiB process cap, and a 2400 second infrastructure timeout. The upstream per-case 5000 ms budgets were unchanged:

```sh
python3 scripts/effect-upstream.py \
  --workspace <owned-workspace> --out <owned-output> \
  --engine native --path Hash \
  --kiln <frozen-cli> --provider-manifest <ABI-82-manifest> \
  --jobs 1 --runtime-jobs 1 --native-profile native-release \
  --timeout 2400 --kiln-memory-mb 4096
```

The run registered all 25 original cases and recorded 24 passes, one failure, zero skips, with no inventory or mode mismatch. The sole failure was:

```text
Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits
Error: TEST_TIMEOUT: 5000ms
```

Node/Vitest expects this case to pass, so the report records one outcome mismatch. The native file process exited 1; the runner did not classify the file itself as timed out. This result does not show that the value ABI had no benefit elsewhere; it shows that this original deadline failure remains.

## Build cost and overlap

The runner compiled the Hash native artifact successfully in 699.99 seconds wall time. Its build report records:

| Phase/artifact | Recorded value |
| --- | ---: |
| Generated C | 184,083,524 bytes |
| Aggregate app object | 167,290,912 bytes |
| Prelude object | 2,815,888 bytes |
| Runtime object | 16,288 bytes |
| Launcher object | 1,584,584 bytes |
| Executable | 115,893,736 bytes |
| Emit | 21,477 ms |
| Compile | 674,214 ms |
| Link | 247 ms |

Other focused compiler/test jobs started after the initial idle check and overlapped this build. These figures are retained as runner-reported cost evidence, not as an isolated build benchmark. The Hash case outcomes retain their upstream budgets and are correctness evidence, not comparative timing evidence.

## Durable raw artifacts

The complete report, per-file JSON, raw stdout, and build log were retained outside Git at:

```text
~/Work/kiln-computed-call-prepared/.scratch/hash-callval-replay/out/
```

Recorded hashes:

- `report.json`: `fd4452fda40a7ef5e02e1a11706512fb7c7180b2f0711d09efa7774ab0717be8`
- `Hash/native.json`: `0becafb6eb508dad216f47a489e2606dc7bb2c4d58d762041fde935e1773edcc`
- `Hash/native.log`: `04cb9ce00a28bcc43c3e6f8c4b08b71f591bac42ce0748483f8bad6c42dd8e80`
- `Hash/native-build.log`: `794df2271773311e27a736fd1175ec4aef6c3b4857435ac4207a21015b92e73a`
- Executable: `8be82a086a0be7ce13948f9c6912fb9e3021592d83b1018ec3a285ecc17c12e2`
