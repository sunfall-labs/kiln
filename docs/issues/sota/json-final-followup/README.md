# JSON final follow-up profile

These artifacts are from the reserved quiet slot using the prepared bulk `encoding_rs` adapter
candidate `matched_timing-4ef85e16328094f3` and the unchanged baseline
`kiln-json-baseline/target/release/deps/matched_timing-1dc06dc969ffc946`. The candidate already
used `convert_str_to_utf16`; the subsequent fused candidate removes the separate `from_utf8`
validation and is not represented by these timing files.

The three paired runs are `baseline-pair{1,2,3}.out` and `candidate-pair{1,2,3}.out`. The sampled
long and Unicode stacks are `candidate-long.sample` and `candidate-unicode.sample`. Long ASCII
was dominated by the shared `string_node` raw scan (3,152 of 3,231 samples); the arena copy and
`memmove` accounted for 75 samples. Unicode had 810 samples in `std::str::from_utf8` and 866 in
`encoding_rs::convert_str_to_utf16`, which motivated the fused validate-and-convert candidate.
