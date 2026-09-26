# 02. The parallel/sequential differential harness

Status: ready-for-agent

Every tier is held to one property: the parallel result is exactly the sequential one. Build the
harness before extending any tier.

- A corpus, `conformance/native/parallel-*.ts`, of callbacks at every tier (qualifying and not),
  each run through `parallelMap` and through `map`, printing both and asserting they are identical,
  including which exception is thrown when several elements throw.
- A property test that generates callbacks and inputs from the qualifying grammar of each tier and
  compares parallel and sequential results natively (`proptest`, as ADR 0060 already uses).
- Each probe also runs under `KILN_GC_STRESS` (`mise run native-stress`), since tiers 2 and 3
  change what the collector must tolerate.
- Each probe runs at `KILN_PARALLELISM=1`, `2` and the machine's core count, and must print the
  same bytes at each.

## Acceptance

- `scripts/native.sh` runs the parallel probes; a mismatch fails it.
- The property test runs in `mbx test -p kiln-backend-c`.
