# Native graph transfer refuses a large-string graph under GC stress

Status: ready-for-agent

At the baseline, `conformance/native/actors-direct-large-string.mjs` passes ordinary parity but diverges with
`KILN_NATIVE_GC_STRESS=1`: the interpreter succeeds, while native exits 1 with
`DataCloneError: graph transfer needs ownership proof`. The probe requires a large-string graph
to move, a retained alias to refuse, and the same graph to move after that alias is released.

The failure reproduces with main commit `3013038a` and its original provider, and with milestone
`340a7d88`. A same-provider control also fails with main’s compiler. This establishes an existing
stress failure; it does not diagnose the precise ownership witness or prove a memory-safety fault.

The complete 645-probe milestone stress run retained 594 matches, 5 refusals, 45 explicit stress
skips and this one divergence. The routine gate passed, including 639 ordinary native matches,
6 refusals and zero divergences. Full GC stress for that milestone must continue to be reported as failed.

Run the dedicated runner without changing case behavior:

```sh
KILN_NATIVE_GC_STRESS=1 scripts/native.sh conformance/native/actors-direct-large-string.mjs
```

Diagnose the failed ownership authority, preserve legitimate retained-alias refusal, then verify
the released-alias retry and the complete native stress corpus. Do not skip the probe or weaken
ownership certification to accept a graph with a real sender alias.

Binary/provider identities and complete retained output are in [evidence.json](evidence.json).

## Authority localization

A diagnostic build based on `340a7d88` records a clear first request, followed
by `KILN_C99_GRAPH_MACHINE_ALIAS` for both the retained-alias refusal and the
released-alias retry. Both refusals encounter the same raw pointer at word
index 328 in the captured machine span. A debugger frame inventory places
that slot inside the provider Rust `kiln_realm_drive` stack frame. The exact
local owner and its liveness remain unproven; do not exclude this span or
weaken certification on this evidence alone.

`machine-witness-evidence.json` records diagnostic source/binary digests and
retains full raw and debugger logs. These diagnostic prints are confined to an
isolated worktree and are not integrated into production.

## Managed boundary fix and focused verification

A watchpoint installed at the first request shows that the provider-stack word already contains
what later becomes the next candidate address, and never changes between the clear request and
the refused requests. Allocation reuse makes an old executor-stack word look like a new alias.
The watch log is retained under `managed-boundary/`; this observation alone is not permission
to drop machine roots.

The combined candidate scopes proof only at the managed provider-drive boundary with installed
graph roots. The noinline C wrapper captures caller registers before entering the Rust scheduler,
then retains the older caller stack above its frame record. The paused scheduler's implementation
frames are outside this span; graph roots and registered host owners remain independently checked.
The wrapper pops its TLS scope on every return. Valid nested scopes select the inner boundary;
invalid or overflowing scopes suppress managed proof until their matching pops. Unverified frame
ABIs and active generated language frames use the original full scan. x86-64 and AArch64 are the
only frame-record ABIs enabled for this path.

At compiler source `319b320d1ce5f0a6b718b25cc89a05aaa646ebeb`, the focused C fixture preserves
refusal for a live older caller alias and checks nested boundary restoration, invalid-scope
suppression and active-language-frame fallback. It passes 1/1; emitted-C checks pass 37/37.
The large-string probe now passes under GC stress, including its retained-alias refusal and
released-alias retry. It and two method-call probes pass all three stressed checks and stressed
realm replay at N=1 and N=2. `managed-boundary-reference.json` records exact binary/provider
identities and raw-log digests. The complete stressed corpus remains unverified for this candidate.
