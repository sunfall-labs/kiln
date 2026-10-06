# Gate recovery: reviewed facts and emission baselines

The routine gate at `fc5d866d` found nine stale whole-program facts chunks after
private brands moved out of synthetic string properties. Regenerating all 31
targets preserves every world classification, module path, immutable binding,
function count and function metadata field. The site multiset removes exactly
113 synthetic `NewArray` sites and 226 `SetPropertyStrict` sites naming
`@@private:` keys; it adds no sites. Instruction positions shift accordingly.
`facts-review.json` records the exact tool and baseline digests. The regenerated
baseline passes `kiln-facts --check` for all 14 benchmarks and 17 bundles.

This is structural analysis evidence, not a correctness or performance result.
It changes no Test262 counts or Effect acceptance conditions. Native correctness
failures from the same gate remain separate repairs.
