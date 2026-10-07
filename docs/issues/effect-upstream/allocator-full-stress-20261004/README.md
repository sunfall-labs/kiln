# Full native GC-stress run, 2026-10-04

This is a **failed** full-corpus run, retained as diagnostic evidence. It must not be reported as a passing GC-stress validation.

The native corpus ran all 650 inventory entries with the frozen release CLI and the explicit full-GUI provider manifest, with `KILN_NATIVE_GC_STRESS=1`, `KILN_JOBS=2`, and `RAYON_NUM_THREADS=1`. The terminal summary was **580 matched, 5 refused, 20 diverged, 45 stress-skipped**. There were no failed, broken, timed-out, or interrupted probes. The five refusals are printed in the full outer log; the 45 skips are the runner's intentional GC-stress skips.

The 20 divergences were all HTTP probes:

- `http-client-end-reentry`
- `http-client-response-listener-order`
- `http-client-socket-preconnect`
- `http-connection-destroy-before-request`
- `http-incoming-readable-pause`
- `http-upgrade-pipelined-head`
- `http-upgrade-write-fences`
- `net-allow-half-open`
- `node-http-write-head-message`
- `app-bun-serve-global`
- `app-http-client-url`
- `app-http-request-lifecycle`
- `app-http-service`
- `app-serve-fetch`
- `app-serve-header-list`
- `fetch-live-response-cancel`
- `http-server-fin-response-policy`
- `node-http-unreferenced-server-receiver`
- `node-response-end-callbacks`
- `web-fetch-redirects`

Each recorded HTTP native invocation exited 1 with `kiln: an argument was not valid`; its interpreter counterpart passed. Source inspection explains the common cause: `FULL_GUI_FEATURES` in `crates/semantics/kiln-native-provider-contract/src/lib.rs` omitted `realm-http`, although `ProviderCapabilities::FULL_GUI` includes `ProviderCapability::Http`. The full-gui packager uses `--no-default-features`, so the archive compiled the `#[cfg(not(feature = "realm-http"))]` linkable HTTP stubs while its manifest advertised HTTP. This run used that unchanged manifest/archive; it did not replace or rebuild the package.

## Provenance

- Source checkout: `fix/effect-allocator-full-stress`, commit `3d41ebb21f316eb4c8b691eb08904eb2bdf4a3d1`.
- Frozen CLI SHA-256: `f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0`.
- Full-GUI provider manifest SHA-256: `3983b20fee7a279a7f76da9dd3ebf204509be5dcef437e88ae190848b6fe9ff1`.
- `scripts/native.sh` SHA-256: `0fef47bb437f658cc03f4f8133a1b25496cbb269defcd9a0904c68990ab6c5d0`.
- Inventory SHA-256: `8e08383f6e86999857d70c6265df41b06266fb8f9fb883cfdcda9a83b3684cc0`.
- Full outer log SHA-256: `5bf22890488f2fc59f745bc1e0ae863d92c2befd3a3383aad9be70165d69d0a6`.
- The full command, start time, and source artifacts are recorded in `provenance.txt`.

The run crossed the 8 GiB free-space threshold twice. Only this worktree's `.kiln/cache` was collected, using the frozen CLI's cache GC, with grace-protected records retained. Both outputs are included as `owned-cache-gc-*.log`; no other worktree's caches or outputs were touched.
