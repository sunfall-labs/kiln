# Native graph transfer refuses a large-string graph under GC stress

Status: ready-for-agent

`conformance/native/actors-direct-large-string.mjs` passes ordinary parity but diverges with
`KILN_NATIVE_GC_STRESS=1`: the interpreter succeeds, while native exits 1 with
`DataCloneError: graph transfer needs ownership proof`. The probe requires a large-string graph
to move, a retained alias to refuse, and the same graph to move after that alias is released.

The failure reproduces with main commit `3013038a` and its original provider, and with milestone
`340a7d88`. A same-provider control also fails with main’s compiler. This establishes an existing
stress failure; it does not diagnose the precise ownership witness or prove a memory-safety fault.

The complete 645-probe milestone stress run retained 594 matches, 5 refusals, 45 explicit stress
skips and this one divergence. The routine gate passed, including 639 ordinary native matches,
6 refusals and zero divergences. Full GC stress must continue to be reported as failed.

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
