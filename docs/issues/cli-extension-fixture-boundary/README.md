# CLI extensions-off fixture project boundaries

The `extensions_off` tests create independent temporary projects beneath the system
temporary directory. On this machine that directory is `/var/tmp/kiln`, which
itself has a `.git` ancestor. Without a nearer project marker, `find_project_root`
correctly treats the ancestor as the project root, so concurrent test fixtures are
scanned as one source tree. A sibling fixture can then be removed while another CLI
process is synchronizing its inputs, producing misleading “no such file” failures.

`Fixture::new()` now writes an empty `package.json` in each fixture root. This gives
each test the project boundary that other CLI fixtures already provide; it does not
change project-root discovery or weaken any assertion.

## Evidence (2026-10-06)

The old prebuilt `extensions_off` test binary passed all four tests serially, but
failed with three test threads and reported paths from sibling temporary roots. The
standalone reproducer recorded in this directory showed the same race without a
marker and successful isolated runs after adding one. With the patched source, the
four-test CLI target passed with both one and three test threads.

The root-gate run that exposed the issue was resource-contaminated by low disk space;
its raw log is retained at the path listed below. The independent prebuilt-binary and
marked-fixture tests ran after space recovered. The attempted cold build under the
wrong Rust 1.99 toolchain was stopped and is retained only as an incomplete attempt
in the agent scratch directory; it is not counted as test evidence.

| Check | Result | Raw record |
| --- | --- | --- |
| Prebuilt binary, serial | 4 passed | `prebuilt-serial.log` |
| Prebuilt binary, 3 threads | Race reproduced | `prebuilt-3threads.log` |
| Direct CLI reproducer, no marker / marker | Race / isolated success | `manual-race.log`, `repro_extensions_fixture_race.py` |
| Patched source, Rust 1.97.1, serial | 4 passed | `marked-serial.log` |
| Patched source, Rust 1.97.1, 3 threads | 4 passed | `marked-parallel-3.log` |

The source base was `6e74e6ad4fa10faf4a6c2fa5ee3e63aae5c0e2e6`; the only production
test-source change is the project marker in `crates/tools/kiln-cli/tests/extensions_off.rs`.
Both patched checks used:

```sh
mbx test -p kiln-cli --offline --test extensions_off -- --test-threads=1
mbx test -p kiln-cli --offline --test extensions_off -- --test-threads=3
```

with `RUSTUP_TOOLCHAIN=1.97.1`, `MBX_GC_AUTO=false`, and all compiler/job limits set to one.

## Digests

```text
marked-serial.log       f3ecdd339ec159ed5ba8fc1d5b583d6a46ccc65852c7406500d8acb28cb99b13
marked-parallel-3.log   1abef6cd81b4ff77cc88df6ebbe90d7906c152247a4e94b094a2a15543295d3d
manual-race.log         40fb4043a1aaa3c97b07222ec3d82ad7630984affa31246541a76565ce730f35
repro_extensions_fixture_race.py
                        cbb0b61c7a137dbb4091d6b1bbbd085e12e64f27d93c6b3ef82df979eba53c68
```

The original gate log is `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/.scratch/final-native-milestone/gate-budget12/evidence/run.sEzluRWS/02-crate_tests.log.raw`.
