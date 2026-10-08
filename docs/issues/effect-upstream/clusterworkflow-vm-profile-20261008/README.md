# ClusterWorkflowEngine interpreter profile (2026-10-08)

This is a whole-file diagnostic profile of the unchanged 47-case `cluster/ClusterWorkflowEngine` interpreter run, not an acceptance measurement and not a per-case profile. The original five-second case timeout stayed unchanged; the outer file cap was 600 seconds. The run completed all 47 cases in 192.452 seconds: 36 passed and 11 timed out. It exited 1 because of those test timeouts. A separate unprofiled run was incomplete at its 180-second outer cap (35/47), so it is not used here.

The measured executable is `kiln` SHA-256 `77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e` (ELF code ID `e1f254bffa8625f39c20ea2de69221a348ab946b`, Breakpad ID `BF54F2E186FAF3259C20EA2DE69221A30`). Its retained build receipt identifies source commit `9de2c761d48d6f86ddee116e1ae1fc90abd203a1` and a clean 651-file build-input snapshot. The profile library code ID matches this executable. The CLI source was interpreter-only; no native provider was selected. It used the pinned Effect source commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`, wall clock, one runtime worker and a 4096 MiB process cap. The test source SHA is `6705d76eb0b90405b51d49ab4c8fbb185f0b306ec5fb909bba406c31f0ca47b5`; the generated entry SHA at the in-flight checkpoint is `02f24e608ccbb680d2a7ecea488ed8c7eda102dbe5d7a2bf79a11a1ac64e0da0`; harness SHA is `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`. The checkpoint was captured during the run, not before launch; the runner report contains the final source and harness hashes. No provider archive applies to this interpreter run.

Samply profile metadata reports a 10.101 ms sample interval (about 99 Hz). The exact sampler wrapper command was not retained as a separate launch receipt; the runner command is preserved in its report. The kiln worker thread contributed 12,828 samples and 129.357 sampled CPU seconds over the 192.452-second runner wall interval. Categories below assign each sample once by its leaf frame; symbol-based buckets use demangled ELF/DWARF names, with a small unresolved remainder. They are descriptive classifications, not causal cost estimates.

| Exclusive leaf category | Samples | Share | Sampled CPU seconds |
| --- | ---: | ---: | ---: |
| `other-kiln` | 6,081 | 47.404% | 61.256 |
| `Rust-allocation-clone-drop` | 2,310 | 18.007% | 23.304 |
| `system-libc-loader` | 1,959 | 15.271% | 19.857 |
| `VM-instruction-call-generator-dispatch` | 1,172 | 9.136% | 11.841 |
| `VM-property-key-lookup` | 1,052 | 8.201% | 10.545 |
| `VM-cycle-GC-owner-graph` | 241 | 1.879% | 2.421 |
| `unresolved-symbol` | 13 | 0.101% | 0.134 |

The largest *individual* leaf frame is `run_bounded_inner` at 1.785% of samples; other large leaves include glibc free internals (1.52%), a hashbrown probe instruction (1.497%), libc chunk unlink (1.333%), `memcmp` (1.255%), `PropertyKeyRef::matches_classified_slot` (1.193%), and another hashbrown instruction (1.045%). The owner-graph/cycle-GC category is 1.879% of exclusive leaves. This is not a single-hot-helper profile: the broad `other-kiln` bucket is 47.404%, and the explicitly unresolved leaf-address share is 0.101%. Do not infer that allocation, GC, or property reads alone caused the timeout.

Top 25 exclusive leaf frames (addresses are in the matching ELF; symbols are demangled):

| Samples | Share | Leaf frame | Image |
| ---: | ---: | --- | --- |
| 229 | 1.785% | `<kiln_interpreter::vm::Vm<kiln_runtime::CaptureHost>>::run_bounded_inner (.llvm.16259537075620108916)` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 195 | 1.520% | `_int_free_create_chunk` | libc.so.6 |
| 192 | 1.497% | `core::intrinsics::likely` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 171 | 1.333% | `unlink_chunk` | libc.so.6 |
| 161 | 1.255% | `__memcmp_avx2_movbe` | libc.so.6 |
| 153 | 1.193% | `<kiln_interpreter::object_model::PropertyKeyRef>::matches_classified_slot` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 134 | 1.045% | `core::core_arch::x86::sse2::_mm_movemask_epi8` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 107 | 0.834% | `tcache_get_n` | libc.so.6 |
| 97 | 0.756% | `unlink_chunk` | libc.so.6 |
| 91 | 0.709% | `kiln_builtin_surface::find_prototype_member::{closure#0}` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 85 | 0.663% | `kiln_interpreter::vm_cycle_gc::object_children` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 82 | 0.639% | `<kiln_string::Repr as core::clone::Clone>::clone` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 82 | 0.639% | `<kiln_interpreter::vm::Vm<kiln_runtime::CaptureHost>>::run_bounded_inner (.llvm.16259537075620108916)` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 77 | 0.600% | `core::intrinsics::unlikely` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 73 | 0.569% | `_int_malloc` | libc.so.6 |
| 62 | 0.483% | `<[u8] as core::cmp::PartialEq>::eq` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 60 | 0.468% | `<kiln_string::StringUnits as core::cmp::PartialEq<str>>::eq` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 57 | 0.444% | `<kiln_interpreter::vm::Vm<kiln_runtime::CaptureHost>>::run_bounded_inner (.llvm.16259537075620108916)` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 55 | 0.429% | `unlink_chunk` | libc.so.6 |
| 55 | 0.429% | `<usize as core::slice::index::SliceIndex<[kiln_ir::Instruction]>>::get` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 55 | 0.429% | `<usize as core::slice::index::SliceIndex<[bool]>>::index` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 54 | 0.421% | `_int_free_merge_chunk` | libc.so.6 |
| 54 | 0.421% | `core::ptr::write::<kiln_interpreter::runtime_value::Value>` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 54 | 0.421% | `core::mem::replace::<usize>` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |
| 53 | 0.413% | `core::ptr::drop_glue::<kiln_interpreter::runtime_value::Value> (.llvm.16259537075620108916)` | kiln-77d260f7eb852b8cdd6fcbc1042614a83e6f3a9927663b7a8860663e731a195e |

The repeated VM stack pattern includes nested `invoke -> invoke_dispatch -> resume_generator -> run_bounded -> run_bounded_inner` calls. `resume_generator` appeared in 6,348 of 12,828 sample stacks, with at most five such frames in one stack. Several common stacks contain multiple nested `run_bounded` frames. This establishes that active work often runs through nested generator resumes; it does not establish pathological recursion. `Effect.gen` is implemented with `fromIteratorUnsafe` and necessarily advances its generator, so these frames are also expected for normal Effect execution. The samples cover the complete file, not one timeout body.

The profile supports that the worker spent substantial CPU executing the interpreter, rather than sitting wholly idle: 129.357 sampled worker CPU seconds in 192.452 seconds of wall time. It does not distinguish the tests' real workflow algorithms from general VM instruction/call/property and allocation work. Runner output has no per-case timestamps, and the sampler profile has no case markers, so failed-case-only sample attribution is unavailable. One timeout moved between diagnostic runs: this profiled run timed out `executes, resumes, deduplicates, and polls a suspended workflow`, while a prior failure was a different case. Treat the exact 11 timeouts as diagnostic outcomes, not as a stable changed failure set or performance verdict.

The source paths printed by symbolication can point into other worktrees because DWARF records build-time paths. Function names and addresses are tied to the measured ELF code ID above; those source path strings alone are not source revision evidence. `raw/analyze_exclusive.py` maps each sample to its leaf frame, resolves frames against the profile's library table, and emits the one-count-per-sample categories and top 25 leaves. `raw/stack_shapes.py` summarizes sampled call chains. The raw profile and samply symbol map are included for reanalysis; the 310,636,712-byte executable is not, but its SHA and build receipt are recorded above.

The profile started at `2026-10-08T17:47:36.456Z`. The in-flight input checkpoint at `17:50:05Z` recorded the pinned source commit and test/harness/entry hashes while the runner lock was held. After completion, the runner and kiln PIDs observed during capture (1673243 and 1673388) were absent from `/proc`. The retained material does not include a separate supervisor process-group cleanup receipt or a prelaunch input snapshot; neither is claimed. This is the cleanup and provenance limit of this diagnostic capture.

`raw/runner-report.json` and `raw/interpreter.json` preserve the exact case results and command; `raw/interpreter.log` preserves stdout; `raw/clusterworkflow.json.gz` is the original profile; `raw/clusterworkflow.json.syms.json` is its symbol map; `raw/input-checkpoint.json` records the explicitly in-flight checkpoint. `raw/exclusive-symbolized.json` contains the exact leaf attribution. `SHA256SUMS` covers all retained files.
