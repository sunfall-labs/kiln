# Bounded loop-callee O1 tier

This candidate adds a separate, bounded graph pass for generic-only modules that are too large
for the existing direct-call analysis. It consumes only `known_callees` results, so reassigned or
otherwise uncertain callable values do not contribute. The pass does not add direct calls, typed
entries, or clones; selected functions retain their boxed generic ABI and only receive the
existing O1 body attribute.

The analysis is admitted only for at most 512 functions and 32,768 body-plus-entry IR
instructions. It profiles at most 256 eligible loop callers, 4,096 known call edges, and four
propagation edges. The supplemental tier selects at most two functions, each at most 1,536 IR
instructions, with 2,048 IR instructions total. Selection weights each eligible body by the
known loop-call context reaching it, then estimates repeated work as context weight times body
size. Ties are deterministic. Liveness is applied later through `set_available`, which filters
unavailable callers and callees before selecting the budget.

On the pinned `SchemaBinary.ts` source, the ignored selector diagnostic reported 295 protos,
25,550 body instructions and 1,719 entry instructions. It found the in-loop `encodeArray` calls
at PCs 209 and 275 targeting `encodeValue` (fn165), and `decodeArray` at PC250 targeting the
`decodeValue` wrapper (fn199); bounded propagation also credited `decodeValueUnchecked` (fn200).
Their loop-call weights were 766 for fn156/fn165 and 387 for fn193/fn200. Before module-liveness
filtering, the two-body tier selected `isCyclic` (fn118, 999 IR) and `encodeValue` (fn165, 1,028
IR), 2,027 IR total. `set_available` reapplies the same bounded policy after liveness. This
selection is an estimate from static loop context: it can credit cold branch callees such as
`isCyclic`; it is not a runtime profile.

Cycle-module AOT emits separate declaration-instantiation and evaluation entrypoints. The
instantiation bytecode ends at instruction 920, before the later immutable binding stores that
make the loop-callee edges visible to the full-module ranking pass. Replanning on that prefix
selected fn91/fn99 and assigned fn165 weight zero; the full-module profile assigns fn165 weight
766. The emitter now computes this bounded ranking profile once from the original module and
shares only those ranking weights with both split entrypoints. Each entrypoint still computes its
own liveness, availability, and known-call map, so a ranking hint cannot create a static/direct
call or retain a body that that phase cannot emit. A regression covers an emitted cycle-module
instantiation body with a later immutable callback binding and a reassigned mutable control.

The pinned Schema diagnostic also calls the production split-module emitter and checks the actual
instantiation C for fn165: it must not carry the mixed-tier O0 attribute. Calls remain generic
boxed calls. This verifies that ranking reaches real C emission rather than only the planner.

Validation ran in the owned worktree based on `9b6d7041725462c70784d6f1234ba815f04a0009`
with Rust 1.97.1 and `mbx`, one Cargo/Kiln/Rayon worker, and the pinned Effect workspace at
`~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace/source`.

- Full `kiln-backend-c --lib` suite: 345 passed, 0 failed, 4 ignored in 105.88 seconds.
- Pinned Schema selector/emission diagnostic: 1 passed; it is ignored in normal test runs and was
  run explicitly with `--ignored`. It includes the actual split-module C assertion described above.
- `cargo fmt --all -- --check` and `git diff --check` passed.

An earlier full-suite attempt without an explicit pinned-toolchain `PATH` was discarded: it selected
Rust 1.99. The accepted full-suite and Schema-emission runs explicitly set `PATH` and
`RUSTUP_TOOLCHAIN=1.97.1`; `rustc --version` reported `1.97.1 (8bab26f4f 2026-07-14)`.

Full raw logs are retained in the owned `.scratch/schema-loop-tier/` directory. The pinned full-suite
log `backend-pinned-final.log` SHA-256 is
`9e3ba7f4d0111aa73eb506f7ad55e5863499266e131540055401b67721ab597b`; the pinned production-emission
diagnostic `schema-emission-proof-1.97.log` SHA-256 is
`39be2d72ba760924a5582eaf0f77c9049387294a11dd520c8e5aa0f90bd760d1`; and the planner diagnostic
log SHA-256 is `76b9d40bfc47145c8f61662ca1acd811fc4e5316d7ae60901479ddc84a568f7f`.
