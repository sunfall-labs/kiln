# Sharding native timeout CPU attribution

The unchanged 66-case native Sharding replay is still 61/66: the same five original
`TEST_TIMEOUT: 5000ms` failures, with no skips or inventory gap. A direct-process `/proc` monitor
shows that these failures are CPU-bound on the native process's only thread, rather than waiting
on a lost wake. The monitor sampled `/proc/<pid>/task/*/stat` every 100 ms, timestamped beside the
program's own case events, and verified the executable path before the first sample.

| Timeout case | Timeout event (s) | Observed thread-state samples | User + system ticks between first/last in-window sample |
| --- | ---: | ---: | ---: |
| active request replay after entity defect | 5.216 | 50 `R`, 2 startup `D` | 484 + 17 over samples at 0.001–5.117 s |
| active uninterruptible request after restarting | 10.225 | 50/50 `R` | 480 + 9 over samples at 5.217–10.135 s |
| active stream after acknowledged chunk | 15.239 | 50/50 `R` | 481 + 8 over samples at 10.235–15.152 s |
| request reset during pending asynchronous storage read | 20.571 | 51/51 `R` | 492 + 6 over samples at 15.553–20.569 s |
| durable messages while entity layers build | 34.133 | 50/50 `R` | 481 + 9 over samples at 29.197–34.117 s |

Linux reports `R` for runnable/running, `S` for interruptible sleep, and `D` for uninterruptible
sleep. Across the full 52.479 s replay there were 495 `R`, 26 `S`, and 2 `D` samples, all for one
TID. In the first timeout window, only the initial two process-start samples were `D`; its remaining
samples and every sample in the other four timeout windows were `R`. The target's
systemd cgroup accrued 49.710 CPU seconds. The per-window tick deltas are about 4.9 CPU seconds over
about 4.9 seconds of in-window samples. These observations support sustained single-core work
through each timeout; they do not identify which operation consumes all of that work.

## Sampled profile limits and source attribution

Two earlier attempts sampled the Python limited-child wrapper and are invalid for target CPU
attribution; their data is excluded here. The preserved third Samply run sampled PID 3413089
directly, and its profile code ID matches the ELF build ID in the retained c810 cache payload and
post-run output file.

The separate Samply replay covered the same five failures. It captured 4,447 samples over 52.12 s,
but reported only 4.44 s of `threadCPUDelta` while the direct-process cgroup accrued about 50 CPU
seconds. Its absolute sample weights substantially undercount target CPU and must not be used as a
CPU-share estimate. Treat the leaf counts below only as the relative contents of the samples that
were captured:

| Failure window | Sharding generated `fn_77` | `kiln_val_to_boundary` | `kiln_c99_gc_lookup_nursery` | `kiln_c99_own_index_key_with_hash` | `kiln_c99_key_hash` |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 30 | 20 | 16 | 18 | 0 |
| 2 | 15 | 11 | 13 | 11 | 0 |
| 3 | 36 | 18 | 17 | 0 | 23 |
| 4 | 22 | 12 | 16 | 0 | 16 |
| 5 | 15 | 10 | 9 | 0 | 12 |

The native symbol sidecar maps `kiln_module_path_d26041571cf61f0bad70365407312980_fn_77` to the
Sharding module. The C translation unit's concatenated `_source[]` is the coordinate domain for
`ku_protos[77]`; the proto span is bytes 34428–38039. `evidence/fn77-generated-source-slice.txt`
contains those 3,611 decoded bytes (SHA-256 `ad75f831fac16cc3ffd4bad4c123ef58baf555cb2dafae71be0924e94a63fe98`). It is the runner-storage
refresh generator ending in `yield* Effect.sleep(config.refreshAssignmentsInterval)`, corresponding
to authored `Sharding.ts:1347`. Directly applying those byte offsets to authored TypeScript is
incorrect because the compiler has generated/concatenated source text. This makes the zero-delay
refresh loop a plausible CPU-work contributor, but the undersampled profile cannot quantify its
share or prove causality. Key hashing, nursery lookup, boundary conversion, and property-kind
checks also appear among sampled leaves; no optimization is justified by this profile alone.

## Binary identity caveat

The runner report `evidence/native-build-report.json` and the linked-artifact cache payload record
the native executable SHA-256 as `c81042e681403161aacf9bbb145e518ae9d9d394875c98a6fa8645941f4d7cfc`.
The process was launched from that reported output path and its live `/proc/<pid>/exe` resolved to
that path. The path was replaced after the monitored run: it now contains the separately frozen
`8ecc0ad00e0162438063c279ec326caf25c08d83e3cfb45b3b9f9992f5ffe839` snapshot in the owned scratch
directory. The retained original linked-artifact cache record is `90ade1285ed0189713bf5416edeb3c7fb183ef012ea5faebf8d894f4f9c99309`; its KILNART payload hashes to the reported `c81042…` executable.

The original cache payload and post-run `8ecc…` snapshot have the same `.text` SHA-256
`00e5de95177451ecedb2ff2666b45d72757508794e8040f7f559d611f1801a90`, the same section virtual
addresses/sizes, and identical bytes in all writable PT_LOAD segments and the executable PT_LOAD
segment. Their first read-only PT_LOAD segment differs only in the ELF `e_phoff` field. The eight
extra bytes and remaining differences are non-loaded trailing metadata. This supports matching
machine code and loaded data. Both files' ELF build ID is `5ff4a0616d9c2745fb5ffeecd925bc6afe02e6de`,
matching the profile's `code_id`. The report does not claim a live-fd hash was taken before the
output path's content changed.

## Inputs and retained evidence

- Pinned Effect source: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Compiler source: `efd9e68a`; immutable CLI SHA-256:
  `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`.
- Native provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
- Original per-case deadlines and assertions were unchanged. This is correctness diagnosis, not a
  benchmark or a final all-corpus verdict.
- `evidence/direct-native.log.gz` and `evidence/per-thread-cpu.tsv.gz` retain the direct replay and
  100 ms per-thread samples. `evidence/samply-profile.json.gz` and
  `evidence/samply-profile-sidecar.json.gz` retain the lower-coverage profile. Status and exact
  upstream runner report are next to them. `evidence/SHA256SUMS` binds the retained files.

The next useful attribution step is a low-perturbation stack capture or instrumentation focused on
the generated refresh loop and its called runtime operations. The present evidence rejects an idle
timer/lost-wake explanation for these five runs, but cannot distinguish expensive finite scheduler
work from a high-frequency loop.
