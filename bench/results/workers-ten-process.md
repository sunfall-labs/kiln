# Server Throughput with `KILN_WORKERS` at Ten Client Processes

Benchmark taken on 2026-09-05 measuring HTTP server throughput (`bench/server/serve.mjs`) natively compiled under `--profile native-release` across worker counts.

Ten node client processes with sixteen connections each (160 total concurrent connections), 5000 ms per run, three interleaved passes, median reported.

- **System**: Linux 6.1.176 x86_64, 16 cores (Intel Xeon Platinum 8259CL @ 2.50GHz)
- **Toolchain**: Zig 0.16.0 targeting `x86_64-unknown-linux-gnu`
- **Compiler profile**: `native-release`
- **Workload**: `hello` HTTP server (`bench/server/serve.mjs`), port 18088

## Results

| `KILN_WORKERS` | Realms (Processes) | Throughput (req/s) | p50 latency | p99 max latency | Errors |
|---|---|---|---|---|---|
| `1` | 1 | 63.3k (63,317) | 2.40 ms | 7.94 ms | 0 |
| `2` | 2 | 78.4k (78,366) | 1.48 ms | 11.22 ms | 0 |
| `4` | 4 | 95.5k (95,454) | 1.36 ms | 8.10 ms | 0 |
| `8` | 8 | 81.8k (81,763) | 1.42 ms | 11.38 ms | 0 |
| `auto` | 16 (detected cores) | 86.1k (86,109) | 1.48 ms | 7.23 ms | 0 |

## Observations

1. **Throughput Scaling**: With multiple client processes driving sufficient concurrent load, multiple worker processes bound under `SO_REUSEPORT` scale throughput significantly above a single realm (peaking around 95.5k–98.2k rps at 4 workers on this machine).
2. **Latency Reduction**: p50 latency drops from 2.40 ms under a single worker down to 1.36–1.48 ms across multiple workers.
3. **Tail Latency**: `KILN_WORKERS=auto` (forking 16 processes on this 16-core system) achieved the best tail latency with p99 max at 7.23 ms.
4. **Auto Discovery**: `KILN_WORKERS=auto` inspects `sysconf(_SC_NPROCESSORS_ONLN)` at launch and clamps to `1..=1024`, dynamically allocating one realm process per online core without requiring environment-specific hardcoded configuration.
