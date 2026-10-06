# Interpreter weak-collection identity index — 2026-10-05

The interpreter previously stored WeakMap and WeakSet entries in a vector. Each lookup,
insert/update, membership check, and deletion upgraded and scanned the entries, making repeated
identity-key access quadratic. Store entries in one `HashMap<WeakCollectionKey, Value>` instead;
the keys still contain only weak references. `ObjectData` boxes the optional map so ordinary
objects pay only the existing optional-pointer footprint. Ephemeron activation and dead-key
pruning iterate the map without upgrading keys into persistent strong storage.

The identity key hashes its variant and weak allocation address (including the Settler reject bit),
while native and callback identities include their realm. No second index is maintained.

Validation in the owned worktree at base `353be6ea5dafd2abead02fb1b7cb8407f3065319`:

- `cargo fmt --check` passed after formatting.
- Focused identity/reinsert test: 1 passed.
- `weak_map_` tests: 6 passed, including ephemeron chains, dead-key/value cycles, live-key retention,
  closure keys, and receiver validation.
- Full `kiln-interpreter --lib`: 272 passed, 0 failed, 1 ignored (273 total), single test thread.

The original pinned Effect Hash workload was then run with the source and five-second case
deadlines unchanged. Its 25 cases produced 24 passes and one timeout:

`Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits`

The other 24 cases passed. The runner reported no skipped or unmapped cases. The remaining case
still hit `TEST_TIMEOUT: 5000ms`, so this change improves the previously reported 15/25 result but
does not fully resolve the Hash workload.

Run provenance: source pin `67ba4e46a11ccda0b6761578bfd22c04ae00167d`; compiler commit
`c47796c671e702414fbf261201b0005ab8be392d`; frozen CLI SHA-256
`b56c9911d80fcd6dfacc06051dde66958c295b79e99f544920ac9d9e8a356de8`. Command:

```sh
python3 scripts/effect-upstream.py \
  --workspace ~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/frozen-clock-native/workspace \
  --out .scratch/weak-index-hash-realclock \
  --engine interpreter --path Hash \
  --kiln ~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/weak-collection-index/target/release/kiln \
  --jobs 1 --runtime-jobs 1 --timeout 1800 --kiln-memory-mb 8192
```

Full runner report, per-file JSON, and log are archived in `hash-run/`.
The copied frozen CLI is `.scratch/frozen/toolchain/kiln-b56c9911d80fcd6dfacc06051dde66958c295b79e99f544920ac9d9e8a356de8` (mode 0444), and its digest matches the build artifact. The exact Hash source SHA-256 is `9fbc376ce19f8de3190eb9de5e8fa5b8bd2698cf16d3617178bf401eb5f3ea0c`; the harness SHA-256 is `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`; and the runner SHA-256 is `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`. The run selected and recorded exactly the 25 listed IDs, with no inventory mismatch.
