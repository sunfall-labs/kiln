# Scripted transport for `ScriptedHost`

Status: ready-for-agent

ADR 0046: "A scripted transport that yields recorded bytes and records what is written gives the
same deterministic conformance run, on paused Tokio time, without a port." Today `ScriptedHost`
answers `take_transport` with `None`, which is what kept the bridged path alive for every
scripted HTTP test.

Add `ScriptedTransport` (`crates/kiln-runtime/src/scripted_transport.rs`): an `AsyncRead +
AsyncWrite` whose reads replay recorded chunks one per poll and then EOF (or stay pending when
the script says the peer keeps the socket open), and whose writes append to a buffer the test
inspects. `ScriptedHost::with_transport(resource, transport)` registers one; `take_transport`
hands it over once; `written(resource)` returns the bytes the engine wrote so far.

The runtime's scripted HTTP tests move from "expect this `TcpRead`, answer with these bytes" to
"these bytes arrive, these bytes must leave". `scripted_provider_drains_response_before_same_turn_server_close`
keeps its name because `scripts/check-http-close-contract.sh` runs it by name.
