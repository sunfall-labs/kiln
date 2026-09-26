# 07. Tier 2: read the heap

Status: ready-for-agent
Blocked by: 05, 06

Callbacks may read arrays (dense and holey), typed arrays, own and inherited data properties of
ordinary objects, and strings (length, code units, comparison), through the no-write read path.
Anything that could run user code stays sequential: getters, proxies, `valueOf`/`toString`
coercions of objects, and exotic objects.

The proof needs to know a read cannot reach user code. Use the value graph's existing type facts,
and fall back to sequential when a receiver's kind is not proven.

## Acceptance

- `items.parallelMap(x => x.price * rates[x.currency])` runs in parallel when `rates` and the items
  are plain data.
- A getter anywhere on the path keeps it sequential, with the reason.
- Every tier 2 probe is clean under `mise run native-stress`.
