# Native inherited module namespace reads: validation receipt

This receipt records the focused validation for source commit
`9059adae32e3cf4582db0695066e179ddc79a53f` (base `353be6ea5dafd2abead02fb1b7cb8407f3065319`).
The change makes generic native property reads resolve a live module-namespace export cell when
the namespace is encountered through a prototype, and prevents the inherited-property PIC from
returning the raw slot. It also preserves TDZ errors on inherited namespace reads.

## Frozen artifacts

- CLI: `~/Work/kiln/.scratch/effect-sandbox-integration/.mbx-targets/v1/5c1de868fb968891fc7f99597e48d1fc52e05ac7131276bf93259032cfc014de/release/kiln`
- CLI SHA-256: `3b1b4936882e864db761d9301bb7b3a67e882b1fe287235f77ea3c775d8401c6`
- Native provider manifest: `provider-4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b/kiln-native-provider.json`
- Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`
- Provider archive SHA-256 (from manifest): `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`
- Provider ABI: 82; target: `x86_64-unknown-linux-gnu`.
- `read.c` SHA-256: `f657c3abb7b6a9fc951d9971dda4f47767856dd02fa444661aa4f0ab552b14c9`
- `prelude_types.h` SHA-256: `89f404575cdeac4893f45862d3eab05cb520fc164c97f2a6c6ab64efaaeafb2c`

## Results

| Probe | Runs / realms | Result |
| --- | ---: | --- |
| `module-namespace-slot-refresh.mjs` | native ordinary | 1 matched, 0 refused, 0 diverged |
| `module-namespace-slot-refresh.mjs` | native GC stress | 1 matched, 0 refused, 0 diverged |
| `module-namespace-slot-refresh.mjs` | realms 1 and 2 | 1 matched, 0 skipped, 0 diverged |
| `module-namespace-slot-refresh.mjs` | GC stress, realms 1 and 2 | 2 matched, 0 skipped, 0 diverged |
| `module-namespace-inherited-tdz.mjs` | Node reference | output `ReferenceError` |
| `module-namespace-inherited-tdz.mjs` | native ordinary | 1 matched, 0 refused, 0 diverged |
| `module-namespace-inherited-tdz.mjs` | native GC stress | 1 matched, 0 refused, 0 diverged |

The inherited TDZ fixture and its cyclic dependency each contain one conformance case. The
slot-refresh fixture contains one case. Realm validation intentionally covered counts 1 and 2;
core-count realm validation was deferred.

The archived files in `raw/` are the complete retained command output for these focused runs,
compressed with deterministic gzip headers. Their SHA-256 hashes are in `SHA256SUMS`. They contain
runner verdict output, not regenerated summaries. No tests or builds were rerun to create this
receipt.

## Fixture identities

- `module-namespace-slot-refresh.mjs` SHA-256: `0bc7f32ed5f435484df8d2559a8fdc85bb9974c4729b3815990d0fb61ccddac3`
- `module-namespace-inherited-tdz.mjs` SHA-256: `b226806468b8f25e1561742280529c3c5bb583eeb82cdd7dd9a33449705db5af`
- `fixtures/module-namespace-inherited-tdz-cycle.mjs` SHA-256: `5d5aeec6ea0e2bd8f3fa9ecd16f8377ce02f6a6d073647d5c9d64da77e3e17b3`
