# Candidate-store lookup memoization candidate

This is an isolated GC barrier candidate on top of `45ac689f` in the owned
`fix/computed-call-prepared` worktree. It does not change heap layout, lookup indexes, or collection
policy.

`kiln_c99_candidate_record_pointer_store` can serve both the old-to-young remembered set and the
active candidate-group ledger for one pointer publication. Previously, when the owner was tenured
and a candidate ledger was open, it looked up the owner and target for the remembered set, then
looked up the target and owner again for the candidate ledger. The candidate now keeps the
validated owner and exact/interior target blocks in locals and reuses them for both consumers. A
test-only compile define counts those calls; no counter storage or increment is emitted in normal
builds.

The reuse is local to one store call. Between the two consumers, the only possible mutation is an
insert into `remembered_owners`, a C ptrset backed by `malloc`/`calloc`. That operation does not
call the collector or user code and does not mutate heap blocks, nursery regions, or candidate
group membership. If exact/interior target resolution returns null, that null is also reused; the
existing cold candidate-large interior search still runs afterward.

The new C fixture proves the direct count and retained behavior: an old owner plus an exact young
candidate target performs two exact membership lookups total (owner and target), updates both
remembered owners and candidate incoming owners, and an interior target performs those same two
membership lookups plus one interior lookup. The fixture also confirms the active-young-owner
fast path remains lookup-free with no candidate ledger.

Focused checks on the candidate source:

| Command | Result |
| --- | --- |
| `mbx test --jobs=1 -p kiln-backend-c candidate_pointer_store_memoizes_membership_lookups_without_losing_edges -- --nocapture` | 1 passed |
| `mbx test --jobs=1 -p kiln-backend-c executable_candidate_ledger -- --test-threads=1` | 10 passed |
| `mbx test --jobs=1 -p kiln-backend-c executable_minor_gc -- --test-threads=1` | 3 passed |
| `KILN_GC_STRESS=1 mbx test --jobs=1 -p kiln-backend-c candidate_pointer_store_memoizes_membership_lookups_without_losing_edges -- --nocapture` | 1 passed |
| `KILN_GC_STRESS=1 mbx test --jobs=1 -p kiln-backend-c executable_candidate_ledger -- --test-threads=1` | 9 passed, 1 failed |

The stress-only failure was `executable_candidate_ledger_observes_post_factory_cell_aliases_and_clears`
at its existing `ROOTS_UNENUMERATED` assertion after clearing a late owner's value. It has not
been reproduced against the unmodified baseline in this worktree, so this note does not classify
it as pre-existing. The candidate's own fixture passes with stress enabled; ordinary candidate
ledger and minor-GC suites pass. No native CLI rebuild, upstream replay, or performance timing was
run, and this candidate is not an acceptance claim.
