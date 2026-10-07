# Combined recovery build

The clean source snapshot d15e9582 built both release kiln and kiln-facts with pinned Rust 1.97.1 through mbx. The receipt records exact frozen binaries, source commit, baseline digest, and the rebuilt full-gui provider. The provider contains all ten declared capabilities, including GUI; the previous gate used the nine-capability package and could not build four GUI fixtures. Packaging verified required exports.

The freshly built facts tool checks the current committed baseline with no regressions across all 31 targets (14 benchmarks and 17 bundles). No additional baseline update was needed after the canonical computed-key change. This is focused verification; the routine gate and full current Effect acceptance remain outstanding.

The combined frozen CLI and full-gui provider subsequently matched all nine selected native recovery probes: array-methods, the four original stack probes, and the four GUI fixtures. No refusals or divergences. The raw summary is native-nine.log.
