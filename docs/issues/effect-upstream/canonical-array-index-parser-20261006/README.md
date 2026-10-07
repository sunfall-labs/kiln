# Canonical array-index parser validation

This change replaces the native C runtime's numeric-provider conversion/round-trip for canonical array-index keys with bounded digit parsing. It preserves the `0` through `4294967294` range, canonical decimal spelling, UTF-16 key handling, and the separate symbol/private-key domains. The reserved-key rejection is deliberate: a symbol such as `Symbol("1")` is not a numeric array index even though its description is numeric.

## Source and artifact provenance

- Source base: public GitHub `main` commit `3c2e9eb5`, with hidden-key fast-reject commit `e57daeda` retained.
- Candidate source before evidence files: commit `cfa7e9c6181c22b19bacc708a7559d7ee2648f45` plus the parser/test/probe changes. The current diff was checked with `git diff --check`.
- Candidate files: `crates/native/kiln-backend-c/src/runtime/properties.c`, `crates/native/kiln-backend-c/src/tests/runtime.rs`, and `conformance/native/canonical-array-index-keys.js`.
- Toolchain: Rust 1.97.1 and mbx 1.22.0; one Cargo/mbx worker, offline.
- Release CLI SHA-256: `4ef50da30940986bd5f8b341d62dd6cd29bdb8c9d5d97e5bbb52187541c9cabb`.
- Number-provider manifest: `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/formatter/toolchain/provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/kiln-native-provider.json`.

## Checks

The focused backend test `canonical_array_index_parser_matches_previous_round_trip_and_respects_domains` passed (1 passed, 353 filtered). It compares byte-string cases against the prior conversion/round-trip behavior and covers canonical boundaries, overflow, leading zeroes, non-digits, UTF-16 and Latin-1 keys, and reserved domains.

The native probe `conformance/native/canonical-array-index-keys.js` matched: 1 matched, 0 refused, 0 diverged. Its Node oracle and native output are byte-identical (SHA-256 `c7141df729d76da8d7ffe1c252aff867d9bf7fd481bceadbd46aead9820414db`). The output includes the `Symbol("1")`/numeric index coexistence case.

Full raw logs are in `evidence/`. Their SHA-256 values:

- `focused-c-test.log`: `81ec859a0099d9b6e5f805828f5c5fdf198343830abadeb7035efeedd8ce6259`
- `native-parity.log`: `e28ff78be08fb06535f3693219a8827b0442036407945a5af20c273c487871ff`
- `node-oracle.log`: `c7141df729d76da8d7ffe1c252aff867d9bf7fd481bceadbd46aead9820414db`
- `native-output.log`: `c7141df729d76da8d7ffe1c252aff867d9bf7fd481bceadbd46aead9820414db`

No timing or broad-suite performance claim is made from these checks.
