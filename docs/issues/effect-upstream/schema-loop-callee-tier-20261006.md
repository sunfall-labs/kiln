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
`isCyclic`; it is not a runtime profile. The native replay remains the evidence for whether this
selection helps the original timeout.

Validation ran in the owned worktree at source base `9b6d7041725462c70784d6f1234ba815f04a0009`
with Rust 1.97.1 and `mbx`, one Cargo/Kiln/Rayon worker, and the pinned Effect workspace at
`~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace/source`.

- Focused generic backend tests: 22 passed, 0 failed, 2 ignored.
- Full `kiln-backend-c --lib` suite: 344 passed, 0 failed, 4 ignored in 108.44 seconds.
- Pinned Schema selector diagnostic: 1 passed; it is ignored in normal test runs and was run
  explicitly with `--ignored`.

Full raw logs are retained in the owned `.scratch/schema-loop-tier/` directory. The full-suite
log SHA-256 is `1d4d2ceb49dca9c1716c8ddcbf8f0b19f0573978656d6d49b9471cabd9fa33be`; the focused log
SHA-256 is `c4c314f79f96521173f650b461c710f6590cb66ef7b15590d20f6851e80b34d4`; and the selector
log SHA-256 is `33481cec776736ef7fa89070c50e0f87b1586d542cb1053077713832000c1f54`.
