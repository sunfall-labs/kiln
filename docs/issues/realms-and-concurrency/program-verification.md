# Concurrency program verification

## 2026-09-29: nursery transfer list membership

Commit under test: `9c917b15`. The frozen-snapshot `KILN_GATE_JOBS=16 mise run gate`
passed all ten steps in 536 seconds. Evidence in the owned graph-ownership-ledger worktree:
`target/gate-logs/run.mkm5mE0u/aggregate.json`; source snapshot
`9b3a61df7648c28165a8820d860c48786dd39b6d5f600e413a59aec97950712f`, CLI
`c679ef527e227866314927c97dcb8904858385e11c20087bdf500df0d1f7d30a`.

The workspace passed 2,289 cases in 90 suites, with zero failures and nine ignored cases.
Native parity matched 359 probes with the existing ShadowRealm refusal and zero divergences.
Node passed 39/39, server passed six with two skips, and regress passed 48/48. Thirdparty
parity, performance and compilation-facts baselines passed. Routine Test262 passed 97,116
cases with no regressions across 122 chunks: 102,522 selected, 116 deferred, 5,020 reported
failures, 382 unsupported and four skips. This is routine baseline coverage.

Before the gate, native every-allocation collector stress passed with 317 matches, one
existing refusal, zero divergences and 42 explicit stress skips. Direct graph, cancellation
and concurrent-stop probes matched at 16 and 64 simultaneous realms, three runs each.
The public direct-graph probe also verifies that moving an escaped alias into an outside
holder after the factory returns blocks transfer, and clearing it permits the same ticket
to retry. Both engines pass. Raw logs are `target/graph-full-list-*.log`.

This change removes unrelated filled-list searches during storage unlink. Candidate discovery
and incoming-alias certification still scan the sender heap; the bootstrap blueprint,
candidate-scaled proof and remaining historical performance checks remain open.

## 2026-09-29: cancellation coverage and captured-array optimization

Commit under test: `a2444203`. The fixed-snapshot `KILN_GATE_JOBS=16 mise run
gate` passed all ten steps in 314 seconds. Evidence:
`target/gate-logs/run.d0J13wjB/aggregate.json`; code snapshot
`2604f6650565b6a9db143ab6929b907c0cc18940c64cd1d8a4a415b91b4d1808`, CLI
`4ac51ae660589d1be48fb08027b8380a354c45d1ead7a899bbaae338c6a035bb`.

The workspace passed 2,288 cases in 90 suites, with zero failures and nine
ignored cases. Native parity matched 359 probes with the existing ShadowRealm
refusal and zero divergences. Node passed 39/39, server passed six with two
skips, and regress passed 48/48. Thirdparty parity and the performance and facts
baselines passed. Routine Test262 again passed 97,116 cases with no chunk
regressions, from 102,522 selected and 116 explicitly deferred cases.

The captured-array probe additionally matches Node and the interpreter in
native-release, including before-initialization calls and a captured parameter
changed while its reader is active. Its native-release executable passes
every-allocation collector stress. Release-C inspection confirms the immutable
reader has an entry guard and mutable, loop-written and parameter readers do not.

An earlier run passed each lane but failed aggregation because a newly tracked
probe and its profile marker changed the snapshot during the run. That run is
not accepted gate evidence. Keep all tracked files and the index unchanged
until aggregation finishes; the snapshot includes tracked documentation too.

Coordinator prepublication cancellation coverage is now verified. The remaining
historical performance audit, broad bootstrap blueprint and scalable direct
graph proof remain open.

## 2026-09-29: socket-capable gate

Commit under test: `e4bda4df`.

`KILN_GATE_JOBS=16 mise run gate` passed all ten steps in 622 seconds in the
owned `bootstrap-density-2` checkout. The aggregate identifies code snapshot
`180a97ff775c3189a5e16e851f86af2c2bf9d20a26e86a69b87335cba884f889` and
CLI binary `2305ad914a8277567eda5f1a3a9187f032632e110960d070990659e681b57303`.
Evidence is retained in `target/gate-logs/run.Lpy7WKIT/aggregate.json` and its
per-step logs and result documents.

| Check | Result |
| --- | --- |
| Workspace | 90 suites; 2,287 passed, 0 failed, 9 ignored |
| Native parity | 358 matched, 1 existing ShadowRealm refusal, 0 divergences |
| Thirdparty interpreter parity | 17 matched, 0 divergences, 5 fixtures not built |
| Thirdparty native parity | 17 matched, 0 refusals, 0 divergences |
| Node corpus | 39/39 passed |
| Server corpus | 6 passed, 2 skipped, 0 failed |
| Regress corpus | 48/48 passed |
| Routine Test262 | 97,116 passing; no regressions across 122 chunks; staging/sm gained 6 |
| Thirdparty performance baseline | Passed |
| Compilation facts baseline | No regressions across 31 targets |

Routine Test262 verified an exact partition of 102,522 selected cases and 116
deferred extended cases from the pinned 102,638-case inventory. This is a routine
baseline result, not a complete Test262 conformance pass. Its 5,020 failures,
382 unsupported cases and 4 skips remain reported in the result document.

This run supersedes the earlier socket-restricted gate attempts. Coordinator
cancellation coverage, historical performance checks and the
broad bootstrap density work remain open; a green gate alone does not close the program.

## Collector stress and simultaneous realms

`KILN_JOBS=8 mise run native-stress` passed on the same code: 316 matched,
1 existing ShadowRealm refusal, 0 divergences, and 42 explicitly skipped large
stress probes. Full output is retained in `target/concurrency-final-stress.log`.
The ordinary gate covers the skipped probes without every-allocation stress.

`scripts/native-realms.sh --repeat 3` passed at realm counts 1, 2 and 16 for
`process-env-sealed-layout`, `actors-direct-graph`, `actors-direct-graph-cancel`
and `actors-concurrent-stop`: 4 matched, 0 skipped, 0 divergences. The runner used
the gate's release CLI and one probe job. Output is retained in
`target/concurrency-final-realms.log`. This is an affected-subset check, not a
fresh whole-corpus simultaneous-realm run.

## HTTP comparison

The three-round HTTP refresh is in
`bench/concurrency/results-2026-09-29-server.md`, measured with the same release CLI
after correctness and stress completed. All seven server configurations completed
without request errors; no cell crossed the runner's 25% spread threshold. Issue 13
records the results and measurement limitations.

## 2026-09-29: candidate-scaled transfer shipping batch

Production admission uses candidate-owner witnesses and independent context/root authorities;
canonical keys and native shapes cannot borrow candidate storage. Strict full-scan audit agrees
on the public transfer subset and fixed-candidate scaling cases. The 100 MB versus 1 KB buffer
handoff acceptance was recorded earlier; this batch completes the incoming graph proof.

Frozen source `6e9cc7a8` passed 2,323 workspace tests in 93 suites, with no failures and nine
ignored tests; native parity matched 362 probes with one existing ShadowRealm refusal and no
divergences. Routine Test262 passed 97,118 of 102,522 selected variants with no regression
across 122 chunks; 116 extended variants remain deferred. Node passed 39/39, server six with
two skips, and regress 49/49. The gate's only failure was a dayjs timing measurement; the isolated
performance lane passed all 12 present bundles on identical source and CLI with the unchanged
baseline. Both results are retained, rather than describing the original aggregate as green.

Full collector stress passed: 318 matches, one existing refusal, zero divergences and 44 explicit
skips. Strict-oracle native parity matched eight affected probes. Those probes matched twice at
1, 2, 16 and 64 realms; a three-probe subset also matched twice at 1, 2 and 16 realms under
collector stress. Source, index and CLI were verified unchanged throughout. Durable records:
`../performance-evidence/graph-ledger-shipping-verification-2026-09-29.json`.

The package test now checks successful closed-graph transfer and rejects reuse of a consumed
ticket, with actor cleanup in `finally`. Rejected entry modules report their original error even
with referenced actors or timers; two CLI regression tests pass, and the original hang reproducer
exits with its assertion error in 0.23 seconds.

Issue 08's broad bootstrap sharing stays deferred. Additional candidate-size and outside-owner-size
curves are performance characterization, not a claim of constant-time proof for arbitrary graphs.
