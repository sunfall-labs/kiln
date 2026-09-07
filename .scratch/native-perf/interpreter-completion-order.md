# The interpreter runs two ready host completions back to back

**Role:** implementer. **State:** open. **Found:** 2026-09-07, reviewing the promise job lane.

`crates/kiln-interpreter/src/lib.rs` (around 10973): a `RuntimeTurn::Completion` is followed by
`while let Some(next) = runtime.take_next_ready_completion()`, so when several host operations
finish in the same turn their callbacks run one after another with **no microtask checkpoint
between them**. node drains the microtask queue after each completion.

Six concurrent `fs.promises.readFile` calls, printing a marker per phase:

    node        R0 R0-then R0-then2 R0-tick   R1 R1-then R1-then2 R1-tick   ...
    native      R0 R0-then R0-then2 R0-tick   R1 R1-then R1-then2 R1-tick   ...   (matches)
    interpreter R0 R1 R2 R3 R4 R5   R0-then...R5-then   R0-then2...   R0-tick...

The native backend matches node. The interpreter does not, so the two engines disagree.

**Why it matters more than it looks.** `scripts/native.sh` uses the interpreter as its oracle, so
a `.cjs` probe with two concurrent completions reports DIVERGES against a native backend that is
in fact right. No probe in the corpus does that today, which is the only reason this is quiet.

The root cause predates the job lane (it is in `9df4dc2d`, 2026-09-05); the lane changed which
wrong order the interpreter produces, from `R0 R0-tick R1 R1-tick ...` to the one above, and is
what made the disagreement visible.

**Fix:** give `drain_microtask_jobs` a turn between completions, the way the promise lane's
two-pass drain does within a turn. **Then** add the probe -- deliberately after the fix, so the
corpus does not carry a known-diverging file.
