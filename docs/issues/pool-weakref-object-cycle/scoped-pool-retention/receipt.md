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

## Aligned current-source diagnostic

This is a private diagnostic probe, not an upstream test or conformance verdict. It kept the
Effect `Pool` acquire/invalidate/shutdown sequence and scope lifetimes fixed while adding only
WeakRef observations and environment-gated interpreter ownership logging.

### Provenance

- Upstream pin: Effect `4.0.0`, `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Upstream `Pool.test.ts` SHA-256: `a219193dd30dc547f7364a723e2682d653ab824ccbda816b48b1a9d5bfb539f8`.
- Probe SHA-256: `dc0e5485b048b7f238d066a790a3681a804c5aa892ba10f5b11629a9f1af90b9`.
- Interpreter source base: `64808deb2c6bd0abfd2458c7e370da5719ffc1a8`.
- Private diagnostic source diff SHA-256: `0a48063136e4bab977cf9aec70af425b5a5c4df786c19c302f5fdf01eec483fc`.
- First aligned CLI SHA-256: `53409c1b984fe6e4517d3178f0bffb8c93b904cecbffc70141a4514cdcb4eb10`.
- Expanded-root CLI SHA-256: `21f03dc6b4373a139f1b109c0f15ca9c1f11201cc119cf7fbb9563ae439f84a1`.

The aligned build used `MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- mbx
build --release -p kiln-cli --jobs=1`. Both diagnostic executions ran the same probe from the
independent pinned source workspace with `KILN_POOL_GC_TRACE=1` and `kiln --one-shot run`.

### Observation

The probe exits successfully. Both invalidated Pool items and their finalizers are dead after GC;
the two resources remain alive. The two corresponding allocation scopes remain alive, have no
parent, are closed, and still have `state.exit.value === resource`. Their acquisition Context
WeakRefs are dead. The current replacement scope remains open and rooted as expected.

In the first aligned CLI trace, the two retained allocation-scope objects each reach
`strong=2,incoming=0` with no owner-graph parents after collection. The expanded-root trace also
checks exact identity in VM stack/locals/arguments/call frames, execution slots, and queued
microtask/timer contexts and callback arguments; it reports no direct root for either scope.
This narrows the owner to a Rust-held reference outside those inspected slots, but does not
identify its owner. No collector behavior or Effect implementation was changed.

The expanded-root diagnostic does not yet include `ModuleMap.weak_ref_keepalive` or
`ModuleRealmShared` direct Value fields; those checks were prepared but not built or executed due
resource coordination. The archived expanded-root binary/log therefore should not be described as
an exhaustive root census.

### Next diagnostic

Use the Scope target pointer from its WeakRef-construction hook, verify the pinned Rust `Rc` layout,
and watch the strong-count field from Scope creation through the first unexplained retained count.
Attribute clone/drop backtraces to the lasting owner before changing collector behavior.
