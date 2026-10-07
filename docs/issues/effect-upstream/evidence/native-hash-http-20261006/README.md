# Native Hash and HttpClient replay

These are sequential native-release acceptance diagnostics for the frozen combined compiler, using the pinned Effect source workspace and provider capsule. Original per-case deadlines and assertions were unchanged. No timing comparison is claimed; native C compilation dominates the elapsed time and overlapped other bounded work.

## Inputs

- Compiler source revision: `efd9e68a`.
- Frozen CLI SHA-256: `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`.
- Effect source pin: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Native provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b` (copy in `provider/`).
- Runner SHA-256: `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`.
- Runtime: Rust 1.97.1 selected through `mise exec`; native profile `native-release`; one compiler/runtime worker; 4 GiB address-space cap with a 2 GiB swap allowance; 2,400 s infrastructure timeout. The harness retained its original per-case 5 s deadline.

## Results

- Hash: 25/25 cases mapped, 24 passed, one failed at its unchanged 5 s timeout: `Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`. This matches the prior unsampled interpreter failure; it is still an unresolved native timeout. CLI-generated binary SHA-256: `8bb6535dd9136e4ffdb999ec2cfed336128ec8f97fe8351d483192d7a5fd47fb`.
- HttpClient: 8/8 passed, with complete Node/Vitest mapping and no mode/outcome mismatches. Binary SHA-256: `590b014add431faf53e1c385f6df72a5d464c3d1d3bfede6a32dbb28161837c7`.
- Sharding: 66/66 cases mapped, 61 passed, 5 failed at unchanged 5 s deadlines; there were no skips or mapping/mode mismatches. The failures were the three claim-release regressions (active request after entity defect, uninterruptible request after restart, active stream after acknowledged chunk), a request reset during an asynchronous storage read, and holding durable messages while entity layers build. Binary SHA-256: `c81042e681403161aacf9bbb145e518ae9d9d394875c98a6fa8645941f4d7cfc`.

The Hash build took 619.565 s overall (emit 21.069 s, C compile 596.930 s, link 0.361 s); generated C was 183,643,974 B and the aggregate app object 167,944,224 B. The HttpClient build took 787.055 s (emit 25.356 s, C compile 759.925 s, link 0.305 s); generated C was 287,400,817 B and the aggregate app object 267,824,440 B. At 11m54s during HttpClient compilation, the temporary build directory contained 563 `.c` files and 562 `.o` files; this is a live snapshot, not a count inferred from the final aggregate report. Its service memory peak was 2.45 GiB. Sharding took 252.715 s to build (emit 21.676 s, compile 229.043 s, link 0.364 s), with 344,876,682 B generated C and a 327,478,840 B aggregate app object; test execution took 52.271 s. The service peak was 2.92 GiB.

Each suite directory retains the raw per-case log, JSON report, full file report, native build summary, supervisor output, and terminal service receipt. Native binaries are intentionally excluded; their exact SHA-256 values are recorded above and in the reports.

An initial selector typo (`Sharding`) was rejected by the runner before tests started; its invocation receipt is retained under `Sharding-selector-validation/` and is not counted as a run. The correct pinned selector was `cluster/Sharding`.

`SHA256SUMS` covers every retained file except the manifest itself.
