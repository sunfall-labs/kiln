# A JSON object key parsed past depth 32 can come back as another key's bytes

**Role:** implementer. **State:** open. **Found:** 2026-09-07, reviewing the JSON build work.
**Repro:** `.scratch/native-perf/deep-json-keys-repro.mjs`, run natively under `KILN_GC_STRESS=1`.

`JSON.stringify(JSON.parse(x))` stops being a round trip for objects nested deeper than 32 when
the keys are all different:

    kiln run --native .scratch/native-perf/deep-json-keys-repro.mjs      # every line round-trips
    KILN_GC_STRESS=1 <the built binary>                                  # depth 40, 64, 200 differ

The keys come back as another key's bytes. Depth 8 through 33 are fine; the failure begins above
`KILN_JSON_BUILD_LOCAL_FRAMES` (32, `crates/kiln-backend-c/src/runtime/json.c:800`), which is
where the build frames leave the machine stack and stop being found by the conservative scan.

**Not new, and today's work made it better.** At `aa9c512c`, this session's base, the same probe
fails at depth 40 and beyond for *both* repeated and distinct keys. On `48ecc4a2` only the
distinct-key case fails. Nothing in the JSON changes that landed touches
`kiln_json_build_frame`, `kiln_c99_json_build_mirror`, `..._materialize_key`, `..._open` or
`..._release`; they are byte-identical across them.

**What is ruled out.** Pinning the key the moment it is interned, immediately before
`kiln_c99_json_define` (`json.c:1036`), does **not** fix it — measured. So the interned block is
not what dies: `frame->key.ptr` already points at recycled bytes by the time delivery reads it.
That moves the suspicion off the key table and onto the mirror: a spilled frame's key is a heap
string kept alive only by `stack.holds` (`json.c:816`), and something about that is not holding.
Worth checking first: whether `holds` is reached by the collector at all once the frames are in
`malloc` memory, and whether writing `held->elements[slot]` directly past a `kiln_c99_array_push`
that may have replaced the elements block leaves the value in the block the tracer walks.

**Why it is quiet.** It needs a collection inside a narrow window, so `KILN_GC_STRESS=1` shows it
every time and an ordinary run essentially never does. `scripts/native.sh` does not run under GC
stress, which is why `conformance/native/json-shaped-build-edges.js` passes there despite already
covering 200-deep nesting. Running the native corpus under stress periodically would have caught
this and would catch the next one.
