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

## Rc-header heap scan (candidate matches only)

The diagnostic CLI was built from source base `64808deb2c6bd0abfd2458c7e370da5719ffc1a8` plus
private instrumentation (source-diff SHA-256
`84c667912d1cfa97dafe6e3429daa7c76e8d3029b45a9ee88a8450787b75ac2a`). The executable SHA-256 is
`9600e8fb7fa255e083042a45b772df4796842a37fa2d3627e8f27347e5e65c1c`; the isolated probe remains
`dc0e5485b048b7f238d066a790a3681a804c5aa892ba10f5b11629a9f1af90b9`.

The exact `Rc<RefCell<ObjectData>>` header is 16 bytes before the allocation payload on this x86-64
build; the birth hook's `Rc::strong_count` agreed with the header's first word. The first scan
attempt attached its conditional stop to the allocation hook by mistake and produced no memory scan;
its full output is `heap-scan/failed-attempt.log`. The corrected run stopped at the first
`WeakRef.deref()` entry observed while the selected Scope header read strong=1, weak=3. It scanned
80 writable mappings and found 12 occurrences of the header bytes in anonymous mappings. The
complete GDB script and output are `heap-scan/scan.gdb` and `heap-scan/successful-scan.log`.

Those occurrences are not yet attributed to an owning `Rc`. `Value` is a Rust enum without an
explicit representation, and `Weak<T>` stores the same header pointer as `Rc<T>`. The scan also
does not distinguish allocator slack, stale freed contents, collector/weak-census storage, or
anonymous thread-stack mappings. Its stop is before a WeakRef upgrade but is not yet proven to be
the WeakRef for this selected Scope. Consequently the 12 matches are leads only; this run does not
identify the retained strong owner or establish a production fix.

## Next diagnostic

The next diagnostic will identify live parent fields using a private, nonowning allocation
registry and type-aware edge inspection. Raw pointer matches alone cannot distinguish a strong
owner from weak handles or stale storage. No production collector change is justified yet.
