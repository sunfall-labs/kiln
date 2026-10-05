# Sharding timeout-window runtime attribution and mixed-tier budget

This is a read-only reanalysis of the archived Samply profile, aligned to the six original
Sharding case windows that ended in the unchanged 5-second timeout. The profile executable and
its limitations are recorded in `README.md`; the diagnostic run was 60/66, while the paired quiet
run was 61/66. Marker intervals are arrival-to-arrival, so they include harness hooks, setup and
cleanup. They identify the body interval broadly, not an exact callback boundary.

## Runtime evidence

The six fail intervals contain 13,856 native samples. Their aggregate top leaf counts are:

| Leaf | Samples | Share |
| --- | ---: | ---: |
| `kiln_c99_invoke_method_dispatch` | 902 | 6.51% |
| Sharding `fn_77` | 633 | 4.57% |
| `kiln_c99_gc_lookup_nursery` | 438 | 3.16% |
| `kiln_c99_key_hash` | 393 | 2.84% |
| `kiln_val_to_boundary` | 385 | 2.78% |
| `kiln_c99_get_own_kind` | 335 | 2.42% |
| `kiln_c99_own_index_key_with_hash` | 267 | 1.93% |
| `ku_call_fast` | 239 | 1.72% |
| `kiln_c99_new_object_sized` | 236 | 1.70% |

The four adjacent passing controls contain 1,020 samples. Their corresponding shares are 2.55%
for method dispatch, 4.22% for `key_hash`, 3.63% for nursery lookup, and 4.22% for Sharding
`fn_77`. These controls are short and are not matched for total work, so the percentages are
context, not a controlled speed comparison. Dispatch is the largest repeatable native leaf in
failure windows, but even eliminating all of it would have a 6.51% Amdahl ceiling in these
samples; the profile does not justify claiming that it explains the timeouts.

The dominant inclusive chain in every failed window is `kiln_c99_invoke_closure_body` →
`kiln_c99_call_inner`, followed by generated Effect/Sharding frames. `run_one_timer` and
`drive_one_ready_turn` are also high in inclusive samples, but not native leaves. In those stacks
they enclose JavaScript timer callbacks, so their inclusive count is evidence that callbacks are
running, not that the Rust scheduler itself consumes that share. Provider-only leaf work is rare;
for example, `kiln_realm_callable_store` appears 15–26 times in each roughly 2,300-sample failed
window. Cause pretty-print functions occur in these test bodies and are not exclusive to timeout
reporting. The repeated timer-callback workload and generic JS call path are the strongest leads;
a new per-method histogram or a focused uninstrumented control is needed to choose a specific
optimization. No runtime change follows from these samples.

`timeout-window-runtime-attribution.json` contains all six failure windows, four adjacent passing
controls, their top 20 native leaves and inclusive symbols, and frequent adjacent stack edges.
`analyze-timeout-windows.py` reproduces it from the archived profile, symbol table and arrival
markers.

## Mixed-tier selector accounting

The profile had suggested optimizing Sharding `fn_77`. Its generated metadata says 916 IR
instructions, 3,611 source bytes, 36 generic binary-operation sites, and 43 property-read sites.
The generic mixed-tier selector has a 320-instruction per-function ceiling, so `fn_77` is
ineligible; it already contributes only 4.57% of timeout-window native leaves.

The selector in `crates/native/kiln-backend-c/src/direct.rs` is a deterministic source-order walk
with caps of 64 functions, 320 instructions per function, and 4,096 selected instructions total.
It also rejects zero-sized functions and functions that are not `direct_callable` (including
frames that must be published and suspending functions). The call site passes the complete
`program` and frame-publication facts to the selector. Reachability is applied later, when the
selected bitmap is combined with the module's reachable-function set for emitted optimization
attributes. Consequently an unreachable proto can consume one of the 64 slots before it is
filtered from emission.

For this Sharding artifact, the emitted `ku_protos` table has 739 rows: 738 emitted bodies and a
null row at index 0. Exactly 63 emitted bodies lack the O0 attribute. This fits the selector's
64-function cap with one selected but non-emitted proto. Proto index 0 is the sole null table row,
so the evidence points to it consuming the remaining selection slot. This is an accounting
explanation, not a proposed behavior change; changing selector reachability requires a separate
correctness and compile-cost review.

Useful source locations in the archived source tree:

- `crates/native/kiln-backend-c/src/direct.rs:1284` defines the 64 / 320 / 4,096 bounded
  selection and its `direct_callable` filter.
- `crates/native/kiln-backend-c/src/direct.rs:182` invokes it on the complete `program` in the
  generic-only plan.
- `crates/native/kiln-backend-c/src/lib.rs:5539` applies the later reachability filter to selected
  generic bodies.
- `crates/native/kiln-backend-c/src/emit_graph/body.rs:490` emits O0 for generic bodies outside
  that filtered bitmap.
- `crates/native/kiln-native/src/graph_source_emitter.rs:15` sets a separate module-level
  generic-only threshold of 512 instructions / 16 functions. Those limits decide whether the
  module is generic-only; they are not the mixed-tier per-function budget above.

This attribution is tied to the exact archived diagnostic executable (SHA-256
`631280990eb04580de181d39244bf08752687e09dbb84bcacbfd7e33693f3245`, Build ID
`9aaf7e609e8cddefecb302bb3d0583d0f990ca5d`) and source/compiler scope in the adjacent `README.md`.
