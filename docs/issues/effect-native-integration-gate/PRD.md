# Effect native integration validation

Status: in-progress

The complete routine gate on source `c84651fc2264a4f6aa16fbbef2d53d5ca5ea8b5c`
failed. Its CLI SHA256 was
`9a170d33a25424e25568b4ac2747d9d15d4025bea572bbd43c338b1a943c651e`;
the recorded source snapshot digest was
`7181a9d1279ad0492ac30464a7cd0c39bd55689d8d20aa2d8ca3a8bb8e47f298`.
The retained aggregate and complete failing-lane logs are under `evidence/`.

Native parity matched 641 probes, reported five existing refusals, and diverged on
`parallel-refused-release.js`. Its grow/shrink map callback mutates the input:
the interpreter keeps the original output length and a hole at the removed index,
whereas the native typed loop returned three dense elements. The probe declares a
stress skip, so the separate successful GC-stress run does not prove this case.

Workspace tests passed 2,594 cases, failed four, and ignored nine across 110 suites.
The failures were the mutable-C-state ownership guard, parallel-site planning for
prepared method calls, the Promise.resolve/await emitted fast-path guard, and the
typed-entry Math.sqrt call-site guard. Each must retain its original architecture
or optimization requirement when corrected.

Routine Test262 selected 102,522 variants, with 97,608 passing and no chunk
regressions; 116 extended variants were deferred. Thirdparty correctness, Node,
server, regress, and whole-program facts checks passed. These results are not a
full Test262 conformance run.

The staged thirdparty benchmark built its 12 selected bundles, then exhausted its
900-second release-marker wait while correctness checks remained alive. It did
not produce timings. The gate took 4,312 seconds with a four-worker total budget;
its cold native parity lane took 2,493 seconds and crate lane took 4,261 seconds.
The release-wait correction distinguishes live owners, dead owners, and aborts,
and gives the gate an explicit longer bound. It does not establish a performance
result or excuse long validation times.

Follow-up integration checks have passed all five liveness tests, 17 focused
collector tests, and all 243 interpreter unit tests (one ignored). The ownership
guard now passes after recognizing the anonymous struct's actual declarator and
classifying the managed drive boundary stack as thread-local frame bookkeeping.
The remaining native failures still require executable and emission evidence,
followed by the repository gate before landing.
