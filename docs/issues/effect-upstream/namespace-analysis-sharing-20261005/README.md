# Share namespace analysis within module selection

Status: candidate

A nonintrusive profile of the debug TypeScript candidate compiler during native Formatter construction recorded 5,643 busy-worker samples. `closed_world::graph_for` appeared in 5,334 inclusive stacks; `build_function_graph` appeared in 5,115. The capture overlapped other work and used a debug CLI, so it cannot establish a release compiler regression or a wall-clock speedup. The attached profile, symbol sidecar, resolved summary and receipt preserve those limits.

Source inspection shows that namespace-member queries build and verify the entry and every function graph for each import slot. The liveness walk cached individual answers, but named exported-namespace forwarding bypassed that cache. The candidate batches import-slot queries once per immutable consumer module during one selection, shares verified graphs within that batch, and retains only compact per-slot answers. Graphs are released immediately after the batch; no process-wide cache or mutable program state is added.

The per-slot alias and escape algorithm is unchanged. Direct eval, ambiguous lexical parents, graph construction/verifier errors and namespace escapes continue to fail closed. A mixed batch test independently checks two named-read namespaces and one escaping namespace, duplicate slots, direct eval, and an empty request. The graph and liveness suites pass 45 tests, including named namespace forwarding through barrels and demand widening after an escape. Five existing backend namespace tests pass on the final source, including captured module bindings and conservative escape behavior.

This candidate still needs release comparison, integration with the TypeScript stage, and the repository gate before landing on main. The frozen full-322 survey remains on its original production binary and is unaffected.


A subsequent release-CLI emission-only sample on the same immutable full Sharding input completed in both versions. The candidate consumed 45.334 user CPU seconds versus 143.979 for the original production CLI. Both parsed 366 modules, compiled 367 IR records, reported the same entry interface/implementation fingerprints, and found no parallel sites. The candidate ran first and the baseline second while other lanes ran; these are bounded CPU-work observations, not a quiet benchmark or proof that every emitted body is identical. Complete logs, timing envelopes, binary/source/input identities and limits are under `release-analysis/`. The combined static-TypeScript/namespace source is now frozen at `39642835` in the independent gate worktree; the routine gate remains in progress.
