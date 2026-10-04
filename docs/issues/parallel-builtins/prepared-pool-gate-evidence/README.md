# Prepared parallel-call gate failures

The frozen full-gate native parity lane reports **642 matched, seven refused, one diverged**. `parallel-alloc-flatmap` diverges at its “one level only” comparison: the first four printed elements match, but the full result comparison fails. First mismatch index and result lengths were not printed by the original fixture, so this evidence does not identify the bad element or prove a flatten-depth error.

`parallel-alloc-budget` and `parallel-alloc-objects` refuse at instruction 120 with `Call callee is unsupported by the native callable model`, receiver tag 11. That tag identifies a method value. Both optimized and generic call dispatch support method values; neither the tag alone nor a depth-limit hypothesis identifies the missing dispatch route.

The enabled prepared parallel-call planner/guard source is present in the root tree as commit `500710b1`, with the same stable patch identity as `f5ac37ec`. The failure is recorded here without attributing it to an unverified field or weakening the probes. These failures must be reproduced, reduced and fixed before this compiler milestone is pushed.

The full raw lane output and its frozen source/binary identities are retained here. This is failure evidence, not an updated full Effect score or a passing gate.
