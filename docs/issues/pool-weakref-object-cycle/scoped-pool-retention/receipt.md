# Scoped Pool retention observation

This is a diagnostic probe, not a conformance result. It mirrors the failing
`releases invalidated resources while at minimum size` case's lifetime shape:
the Pool runs in one outer `Effect.scoped` generator; each of two gets and
invalidations runs in a nested `Effect.scoped`; both finalizers finish before
`collectGarbage`; and the Pool remains reachable through the same collection.
The only added observations are `WeakRef`s and identity comparisons. The probe
does not keep the invalidated items, acquisition scopes, Exits, or resources
strongly across collection.

Run from the pinned Effect source root with the frozen interpreter CLI:

```sh
~/Work/kiln-effect-next-milestone/.scratch/pool-combined-f1-20261004/toolchain/kiln-f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0 \
  --one-shot run packages/effect/test/.probe/pool-owner.ts
```

The command exited 0. Its stdout is preserved verbatim as `result.json`.
The invalidated PoolItems were distinct from the one remaining live item and
both PoolItem WeakRefs cleared. Their finalizer Effects and nested test scopes
also cleared. Both resources remained live; their Success Exit objects and
acquisition scopes remained live, and each retained scope's Closed Exit pointed
to the respective resource. The observed allocation scopes have no parent. The
WeakRefs to their acquisition Context snapshots cleared, while the replacement
item's current acquisition Context and Open scope remained live. The control
object was collected. This narrows the owner search to the detached closed
allocation-scope/Exit path but does not identify which runtime owner keeps those
scopes alive.

Provenance:

- Frozen CLI SHA-256: `f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0`
- Upstream Effect pin: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Unchanged upstream `Pool.test.ts` SHA-256: `a219193dd30dc547f7364a723e2682d653ab824ccbda816b48b1a9d5bfb539f8`
- Probe SHA-256: `dc0e5485b048b7f238d066a790a3681a804c5aa892ba10f5b11629a9f1af90b9`
