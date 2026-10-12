# Security boundary campaigns

This standalone crate keeps Bolero and its instrumentation out of the runtime workspace and
avoids building the compiler to test three input boundaries. Its lockfile pins the campaign's
dependencies. The release workflow runs all four targets before packaging.

| Target | Oracle |
| --- | --- |
| `json_bytes_and_utf16_preserve_tape_invariants` | Arbitrary UTF-8 bytes and UTF-16 units: accepted tapes have exactly one complete root, valid tags and readable strings; owned and borrowed reads agree. |
| `generated_json_agrees_with_independent_reader` | Valid nested objects and arrays serialized by serde_json: values, numbers and UTF-16 strings agree with the independent reader. |
| `http_framing_is_independent_of_transport_chunks` | Arbitrary bytes, mutated headers, binary bodies, chunked requests and pipelines: buffered and streaming decoding agree across transport boundaries. |
| `shared_graph_readers_preserve_edges_and_reject_stale_handles` | Bounded cyclic records, sparse arrays, Maps and Sets: fields and entries remain readable, lookup agrees with iteration, edges stay in bounds, and retired handles cannot resolve after registry reuse. |

Run from the repository root:

```sh
mise run fuzz-security
KILN_FUZZ_ITERATIONS=100000 mise run fuzz-security
```

Each target defaults to 10,000 RNG inputs. Inputs are bounded to 16 KiB for parsers and 512 bytes
for graph construction, so a smoke run has bounded per-case work. Bolero prints a replay seed and
minimized input on failure. Keep findings as regressions in the owning crate, alongside the fix.
The HTTP campaign's empty input found the empty-trailer pipelining regression.

For coverage-guided campaigns, use the installed `cargo-bolero` extension with the pinned Rust
toolchain and run from `fuzz/security` (the `mbx` wrapper passes cargo subcommands through):

```sh
mbx bolero test json_bytes_and_utf16_preserve_tape_invariants --engine libfuzzer --sanitizer address -T 60sec
mbx bolero test generated_json_agrees_with_independent_reader --engine libfuzzer --sanitizer address -T 60sec
mbx bolero test http_framing_is_independent_of_transport_chunks --engine libfuzzer --sanitizer address -T 60sec
mbx bolero test shared_graph_readers_preserve_edges_and_reject_stale_handles --engine libfuzzer --sanitizer address -T 60sec
```

These target the safe APIs that create and read sealed arenas, including surrogate keys and
adversarial indices and handles. They do not import arbitrary mappings or directly exercise the
native C readers. Collector stress and native parity remain separate required evidence for
changes to allocation, rooting or C adapters. The RNG smoke run is not a sanitizer campaign.
