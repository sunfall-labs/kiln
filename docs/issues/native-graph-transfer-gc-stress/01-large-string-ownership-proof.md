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
