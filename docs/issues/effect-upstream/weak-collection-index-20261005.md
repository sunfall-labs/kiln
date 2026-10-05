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

The original pinned Effect Hash workload is the next validation step; its source and five-second
case deadlines remain unchanged. These unit results do not claim the Hash timeout is fixed.
