# Finish the HTTP engine migration (ADR 0046)

Status: resolved

## Why

ADR 0046 put HTTP connections on one Hyper task over a host-owned transport, with protocol events
called straight into the realm. On 2026-09-02 only the *server* path in the native drive was on
that design. The HTTP client path (`node:http` request/get, natively and interpreted) and the
interpreter's server path still ran through the bridged engine: discrete `TcpRead`/`TcpWrite`
host operations, a duplex byte channel, unbounded event channels, a checkpoint loop that pumped
every open connection after every microtask, and a `_in_context` twin of every runtime entry
point so a callback could avoid nesting `block_on`.

Both engines coexisting is the expensive state: the archive routed every HTTP call by "does the
direct engine own this resource", the runtime kept two response-framing implementations, and the
drive still paid `checkpoint_http` per turn for connections that no longer needed it.

## Scope

1. HTTP client connections become Hyper client tasks over a `HostTransport`
   (`crates/host/kiln-runtime/src/http_client.rs`), dispatching response events directly.
2. The interpreter's server and client paths attach through the same engines with a *queued*
   dispatch that the interpreter's checkpoint drains (the adapter shape ADR 0046 names for it).
3. `ScriptedHost` hands over a scripted duplex, so replay stays deterministic on paused Tokio time.
4. Delete what only the bridge needed: `http_transport.rs`, `hyper_http.rs`, `checkpoint_http`,
   `apply_http_actions`, the bridged `RuntimeHttpConnection` table, the `_in_context` family and
   the archive's route-by-owner branches.

Kept: the HTTP close ordering contract (`scripts/check-http-close-contract.sh`), the C ABI
(`kiln_realm_http_*_active` names and the event view), and `ActiveHttpRuntime` as the seam the
interpreter uses (its shape changes; it has one implementor and one consumer).

## Baseline (worktree `agent-a02548ac292ec3275`, HEAD `8aec5b8`, 2026-09-02)

Note: `mbx` was not installed on this host; it was installed through `mise use -g cargo:mbx`
(1.4.1) and is invoked as `/home/coder/.local/share/mise/shims/mbx`. The unit baseline below was
taken with bare `cargo test ... -j 8` before that; every later run uses `mbx`.

    cargo test -p kiln-runtime -p kiln-interpreter -p kiln-cli --offline
      kiln-cli          test result: ok. 123 passed; 0 failed
      cache_gc_without_toolchain   1 passed
      script_hygiene               5 passed
      kiln-interpreter  test result: ok. 141 passed; 0 failed
      kiln-runtime      test result: ok. 110 passed; 0 failed
      (doc tests)       0 passed / 3 passed

    target/debug/kiln test conformance/node --node
      discovered 35  selected 35  pass 35  fail 0  unsupported 0  skipped 0
    target/debug/kiln test conformance/server --node
      discovered 6   selected 6   pass 4   fail 0  unsupported 0  skipped 2
      (s04-fastify, s05-express skipped: packages not installed)

## Issues

| # | Title | Status |
| --- | --- | --- |
| 01 | Client engine: Hyper client task over a host transport | done |
| 02 | Scripted transport for `ScriptedHost` | done |
| 03 | Archive: client path onto the engine, delete the checkpoint loop | done |
| 04 | Interpreter: queued dispatch onto the engines | done |
| 05 | Delete the bridge and the `_in_context` family | done |
| 06 | Verification, ADR state paragraph, NOTES entry | done |

## After (same worktree, branch `worktree-agent-a02548ac292ec3275`, final tree)

    mbx test -p kiln-runtime -p kiln-interpreter -p kiln-cli --offline
      kiln-cli          test result: ok. 123 passed; 0 failed
      cache_gc_without_toolchain   1 passed
      script_hygiene               5 passed
      kiln-interpreter  test result: ok. 141 passed; 0 failed
      kiln-runtime      test result: ok. 98 passed; 0 failed
        (110 before: 15 bridge tests in hyper_http.rs/http_transport.rs and 12 bridged
        lib.rs tests gone; 8 engine tests in lib.rs, 3 client-engine tests, 2 scripted-
        transport tests added; doc tests 3 -> 0, the compile-fail pins went with the type)
    mbx test -p kiln-native-number-ffi --offline -- --skip deterministic_entropy
      test result: ok. 28 passed; 0 failed (the skipped test aborts at baseline too)

    target/debug/kiln test conformance/node --node
      discovered 35  selected 35  pass 35  fail 0  unsupported 0  skipped 0   (11 ms; 87 ms before)
    target/debug/kiln test conformance/server --node
      discovered 6   selected 6   pass 4   fail 0  unsupported 0  skipped 2   (same two skips)

    mbx clippy --workspace --offline      exit 0 (remaining warnings are pre-existing, in
                                          crates this change did not touch)
    cargo fmt --check                     exit 0
    bash scripts/check-http-close-contract.sh    http-close contract: passed
    bash scripts/native.sh                native: 76 matched, 2 refused, 0 diverged

Deleted, against `8aec5b8`: `http_transport.rs` (765), `hyper_http.rs` (1199), `lib.rs`
-2673/+1863, the archive -772/+101, the interpreter -348/+100. Workspace: +3634 / -5915.
