# Sharding profile: generated-body tier and direct-call audit

This is a read-only audit of the improved native Sharding capture. The exact binary, CLI,
compiler-source commit, marker capture, and failed-case inventory are recorded in
`tier-and-cache-evidence.json`; `raw/` retains the compressed samply profile, the matching
symbol sidecar, marker arrivals, and terminal receipt. The symbol sidecar's ELF code ID matches
the retained Sharding executable's Build ID.

The captured run kept the original 66-case inventory and five-second per-case deadlines. It
finished with 61 matches and the same five original timeout failures. The profile contains 4,531
total samples, of which 4,302 are symbolized native samples; those 4,302 samples account for
45.189 CPU seconds, about 99.6% of the child’s measured 45.357 CPU seconds. This is diagnostic
attribution, not an acceptance or timing comparison.

The first 15 generated-C leaf bodies are tabulated with exact sample counts, IR instruction
counts, mixed-tier O0/O1 attributes, and `publishes_frame` flags in
`tier-and-cache-evidence.json`. The tier classification uses each module's **main** generated-C
payload, whose cache fields mark all seven sampled modules `generic_only=true` and
`generic_mixed=true`, followed by the corresponding function body's `optnone` prefix. The
`publishes_frame` column reads bit 64 of that proto's emitted `ku_protos` flags byte. This avoids
misclassifying a split-cycle function from its separate instantiation payload metadata.

The largest inclusive rows are `internal/effect.ts` fn50 and fn48, then `Scheduler.ts` fn18/fn15.
Their inclusive counts are stack occurrences, so those rows overlap. In emitted C, the hot
`runLoop`, `evaluate`, `shouldYield`, and related sites use `CallMethodPrepared`; the generated
path retains receiver/cache checks, guarded body-pointer fast arms, and generic fallbacks. The
plain calls in the reviewed runLoop/evaluate bodies remain `KU_CALL` for imports or computed
values. The O0 `forkUnsafe` body has only metadata/definition references within the same module;
its body makes prepared dynamic method calls. The profile therefore does not demonstrate that
retaining `known_callees` would speed the dominant sampled sites.

There is a bounded reuse opportunity in the planner for non-cycle generic units: when the
legacy direct-call analysis limit is exceeded but loop-profile limits admit the graph,
`known_callees` is already computed to construct `LoopCallProfile` and then discarded. Keeping
that same-phase snapshot could supply boxed known-call hints without another analysis, while
preserving the existing `direct_callable`, frame-publishing, and generic-fallback checks. That
opportunity is not supported as a Sharding hot-path fix by this profile. Split cycle units use a
`LoopCallProfile` override made from the original whole-module program; carrying its `Known`
snapshot into transformed instantiation/evaluation phases would cross phase-specific entry state
and stale `ValueId`s. A phase-safe version would need fresh bounded `known_callees` analysis per
phase.

`analyze.py` recomputes native leaf and inclusive symbol counts from the raw profile and matching
symbol sidecar. For cache-source reproduction, use the seven exact `cache_record` paths in the
JSON file under the original frozen workspace's `.kiln/cache/generated-c`; each is a KILNART
record with an 82-byte header followed by JSON. For the main generated-C payload, the record's
fields 3 and 4 are the `generic_only` and `generic_mixed` booleans and field 2 is the C source.
The recorded C SHA-256 values identify the payloads used for the function and attribute checks.
No executable or complete cache copy is included.

Run the analysis with:

```sh
python3 analyze.py raw/profile.json.gz raw/profile.json.syms.json \
  --tiers tier-and-cache-evidence.json
```

The accompanying `SHA256SUMS` covers each retained artifact and the analysis script.
