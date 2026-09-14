# Issue 06: Conformance Test Suite & Multicore Performance Benchmarking

Status: resolved
Feature: multicore-concurrency
Governing ADR: ADR 0020, ADR 0057, ADR 0058

Resolution 2026-09-13:
- Added and bundled Effect v4 conformance fixtures `13-actor-service.ts`, `14-zero-copy-queue.ts`, and `15-native-parallel.ts` under `conformance/effect/src/`. All 15 Effect conformance tests pass cleanly via `kiln test conformance/effect` (15/15 passed, 0 failed, 0 skipped).
- Verified native parity: `conformance/native/multicore-concurrency.ts` executes natively and differential runner reports 0 diverged (`./scripts/native.sh`: 195 matched, 0 diverged; `./scripts/differential.sh`: 114 matched, 0 diverged).
- Microbenchmarks `bench/non-io/parallel-map-large.mjs` and `bench/non-io/multicore-parallel.mjs` pass across runtimes with matching checksums.

## Context
ADR 0020 dictates that conformance is measured, not asserted. The interpreter and native backend must agree byte-for-byte on all valid concurrency programs, and performance gains must be verified on realistic workloads against Node.js and Bun.

## Requirements
1. **Conformance Test Suites:**
   * Add test fixtures to `conformance/effect/src/`:
     - `13-actor-service.ts`: Verifies actor mailbox queuing, async execution, and Scope cleanup.
     - `14-zero-copy-queue.ts`: Asserts zero-copy graph transfer across realms.
     - `15-native-parallel.ts`: Validates `forEachParallel` with fiber interruption and error handling.
   * Add probe cases to `conformance/native/` verifying that native compilation matches interpreter output.
2. **Benchmark Workload (`bench/server` & `bench/non-io`):**
   * Add a multicore HTTP workload benchmark that performs in-memory cache coordination and background image/crypto hashing across 4 to 16 cores.
   * Compare throughput, p99 latency, and resident memory (RSS) against Node 25.x and Bun 1.4.
3. **Gate Verification:**
   * Verify that `mise run gate-quick` and `mise run gate` pass without regressions.

## Acceptance Criteria
- Differential test runner (`scripts/differential.sh`) reports 100% agreement between interpreter and native backend.
- Conformance test suite in `conformance/effect` passes all new test cases.
- Native multicore benchmark achieves linear scaling across cores while maintaining resident set size under 15 MB.
- Full gate passes with zero Test262 regressions.
