# Prepared method-call lookup cache

Status: implemented in a candidate branch; awaiting parent integration

## Problem

Prepared method calls split property lookup from invocation so JavaScript evaluates the callee
before its arguments. That split had bypassed the native method-call site's lookup cache, causing
the same property to be looked up again through the generic property path. This particularly
affected computed-symbol calls in Effect's `FiberImpl.runLoop`.

## Implementation

The native runtime now exposes the existing method-site lookup/cache stage independently from
invocation. Both ordinary method calls and prepared method calls share that stage. The prepared
path resolves and retains the callee at the original lookup point, before argument evaluation;
invocation uses that saved callee, preserving replacement-during-argument behavior and the
receiver. Emission diagnostic labels use the normal C-string escaping helper.

## Validation

The regression fixture covers saved intrinsic identity when an argument replaces `Math.imul`, an
overridden intrinsic result, non-callable argument evaluation, and quoted/backslashed property
names. The final source candidate passed native parity (1/1), GC-stress parity (1/1), native realms
at N=1, 2, and 16 (1/1), and stressed realms at N=1 and 2 (1/1). The backend C library tests passed
316 tests, with 0 failures and 1 ignored. The separate emitted-C integration target
(`mbx test -p kiln-backend-c --test emitted_c`) passed all 37 tests after reviewing the snapshot
changes: generated module identifiers, bytecode version 17, and prepared-method helper metadata.
The shared view helper has internal linkage; only the generated-code key helper is exported.

## Performance evidence

The bounded-background diagnostic comparison used the same calibrated workload source
`.scratch/computed-call-final/hot-calls.js` (SHA-256
`c12ac30c5faa8328b893141cee1baad3332c3d4b1654ab758eca45498ca6ac6b`) and alternated baseline and
candidate executions three times, pinned to CPU 15. All five checksums matched for every workload.
The native binaries were baseline `f59fc71f9249afde40b8fa42732efd0170898a821f04e257c56d17b34a6b92c6`
and candidate `d22597809948659efcbbac76ecd87b472ff7bb36e0126e0eb2a9f75b7b041ba4`. The candidate
binary predates only the final C diagnostic-string escaping and fixture punctuation additions;
the measured shared lookup/invocation implementation is the same.

| Workload | Baseline median | Candidate median | Checksums |
| --- | ---: | ---: | --- |
| Plain method | 83.44 ms | 80.92 ms | matched |
| Class method | 78.80 ms | 74.31 ms | matched |
| `Math.imul` | 84.63 ms | 70.53 ms | matched |
| Computed symbol method | 142.14 ms | 108.38 ms | matched |
| Fiber-style loop | 139.98 ms | 119.01 ms | matched |

These are diagnostic measurements taken with bounded background activity, not a quiet-window
performance claim. Raw JSONL samples are retained in the ignored `.scratch/computed-call-final/`
directory in the worktree.

## Integrated verification

At source `5014987a7bc611da2c3058b6cb995e80c01c1d78`, the frozen release CLI passed
all five affected native probes, the same five under GC stress, and two representative
probes with 1, 2 and 16 concurrent realms. Those two also passed stressed realms at 1 and 2.
The separate emitted-C integration suite passed 37/37. `integration-reference.json` records
the compiler/provider identities and retained raw logs. These checks do not replace the
full Effect suite or the landing gate.
