# WeakRef targets in ordinary object cycles

Status: investigating

## Problem

The interpreter's cycle collector discovers candidate owners through weak registries for closures,
generators, WeakRef targets, and weak collections. It does not census every ordinary object. An
ordinary object cycle that owns a WeakRef target may therefore be invisible as an incoming owner
when the target is classified.

This is a confirmed general collector gap, but it is not yet attributed as the cause of the three
remaining failures in the pinned Effect `Pool` suite. Pool's removed item graph includes closures,
which may already be found through the closure registry; a Pool fix needs a concrete owner match.

## Reproduction evidence

Source pin: Effect `4.0.0`, commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.

Frozen CLI: SHA-256 `f1c8f22d171e209a929842563e5dd35f2a2226d9c363486497665bcde9189de0`.

The private probe used the pinned `packages/effect/test/utils/gc.ts` helper, which runs eight
zero-delay timer turns with explicit GC. Its control cases verify that an unreachable acyclic
object is collected, a live cycle owner keeps its target alive, and the target collects after the
owner releases its edges. The unreachable two-object cycle remains alive:

```json
{"deadCycleCollected":false,"deadAcyclicCollected":true,"liveCycleCollectedAfterOwnerRelease":true}
```

The probe itself was outside the committed upstream test inventory. This result establishes the
ordinary-cycle discovery gap only; it does not identify the owner of a Pool target.

## Next evidence needed

The scoped diagnostic keeps both invalidated resources alive through their detached, closed
allocation scopes after Pool items, finalizers, and acquisition Contexts are collected. The
aligned interpreter trace reports an unexplained strong owner for those scope objects, with no
owner-graph parent or direct VM execution-slot root. The details and source/binary digests are in
[the diagnostic receipt](scoped-pool-retention/receipt.md), with complete logs alongside it.

The current evidence does not identify the physical owner. Next, watch the Scope `Rc` strong count
from its WeakRef-construction hook, after verifying the pinned Rust `Rc` layout. Map clone/drop
backtraces to the object that continues owning the scope before changing collector behavior. The
prepared ModuleMap/module-realm direct-value checks have not yet been built or run. Do not add a
global ordinary-object census until the Pool owner path and its relevant object kinds are
established.
