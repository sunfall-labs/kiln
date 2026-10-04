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

Map the existing pre-deref Rc-header hits to concrete live Pool/item/scope fields and active
container bounds. Since both `Rc` and `Weak` store the same header pointer, a hit alone does not
classify its ownership. Do not add a global ordinary-object census until the Pool owner path and
its relevant object kinds are established.
