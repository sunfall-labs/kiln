import { Context, Effect, Pool, Scope } from "effect"
import { collectGarbage } from "../utils/gc.ts"

const control = new WeakRef({})
const itemRefs: Array<WeakRef<any>> = []
const resourceRefs: Array<WeakRef<object>> = []
const acquisitionScopeRefs: Array<WeakRef<any>> = []
const acquisitionContextRefs: Array<WeakRef<any>> = []
const observed: Array<{ label: string; reference: WeakRef<any> }> = []
let finalized = 0
const observeItems = (pool: any) => {
  const items = [...pool.state.items]
  return {
    count: items.length,
    matches: itemRefs.map((reference) => {
      const target = reference.deref()
      return target === undefined ? false : items.includes(target)
    }),
    exitMatchesResource: itemRefs.map((reference, index) => {
      const target = reference.deref()
      const resource = resourceRefs[index]?.deref()
      return target !== undefined && resource !== undefined && target.exit.value === resource
    })
  }
}

await Effect.runPromise(Effect.scoped(Effect.gen(function*() {
  const pool = yield* Pool.makeWithTTL({
    acquire: Effect.acquireRelease(
      Effect.flatMap(Effect.context(), (context) => Effect.sync(() => {
        acquisitionContextRefs.push(new WeakRef(context))
        const scope = Context.get(context, Scope.Scope)
        acquisitionScopeRefs.push(new WeakRef(scope))
        return {}
      })),
      () => Effect.sync(() => { finalized++ })
    ),
    min: 1,
    max: 2,
    timeToLive: "1 hour"
  })
  const outerScope = yield* Effect.scope
  observed.push({ label: "outerScope", reference: new WeakRef(outerScope) })
  for (let i = 0; i < 2; i++) {
    yield* Effect.scoped(Effect.gen(function*() {
      const scope = yield* Effect.scope
      observed.push({ label: `innerScope[${i}]`, reference: new WeakRef(scope) })
      const value = yield* Pool.get(pool)
      resourceRefs.push(new WeakRef(value))
      const item = [...pool.state.items].find((candidate: any) =>
        candidate.exit._tag === "Success" && candidate.exit.value === value
      )
      if (item === undefined) throw new Error("missing pool item before invalidation")
      itemRefs.push(new WeakRef(item))
      observed.push({ label: `item[${i}].exit`, reference: new WeakRef(item.exit) })
      observed.push({ label: `item[${i}].finalizer`, reference: new WeakRef(item.finalizer) })
      if (item.finalizer && typeof item.finalizer === "object") {
        for (const key of Reflect.ownKeys(item.finalizer)) {
          const descriptor = Object.getOwnPropertyDescriptor(item.finalizer, key)
          if (descriptor && "value" in descriptor && descriptor.value !== null && (typeof descriptor.value === "object" || typeof descriptor.value === "function")) {
            observed.push({ label: `item[${i}].finalizer.${String(key)}`, reference: new WeakRef(descriptor.value) })
          }
        }
      }
      if (item.exit && typeof item.exit === "object") {
        for (const key of Reflect.ownKeys(item.exit)) {
          const descriptor = Object.getOwnPropertyDescriptor(item.exit, key)
          if (descriptor && "value" in descriptor && descriptor.value !== null && (typeof descriptor.value === "object" || typeof descriptor.value === "function")) {
            observed.push({ label: `item[${i}].exit.${String(key)}`, reference: new WeakRef(descriptor.value) })
          }
        }
      }
      yield* Pool.invalidate(pool, value)
    }))
  }
  const before = observeItems(pool)
  const finalizedBeforeGc = finalized
  yield* collectGarbage
  const after = observeItems(pool)
  const acquisitionScopes = acquisitionScopeRefs.map((reference, index) => {
    const scope = reference.deref()
    const resource = resourceRefs[index]?.deref()
    return {
      alive: scope !== undefined,
      parentIsUndefined: scope?.parent === undefined,
      stateTag: scope?.state?._tag,
      closedExitIsResource: scope !== undefined && resource !== undefined &&
        scope.state?._tag === "Closed" && scope.state.exit?._tag === "Success" && scope.state.exit.value === resource
    }
  })
  const acquisitionContexts = acquisitionContextRefs.map((reference, index) => {
    const context = reference.deref()
    const scope = acquisitionScopeRefs[index]?.deref()
    return {
      alive: context !== undefined,
      pointsToAcquisitionScope: context !== undefined && scope !== undefined && Context.get(context, Scope.Scope) === scope
    }
  })
  console.log(JSON.stringify({
    finalizedBeforeGc,
    finalizedAfterGc: finalized,
    before,
    after,
    itemAlive: itemRefs.map((reference) => reference.deref() !== undefined),
    observationsAlive: observed.map(({ label, reference }) => ({ label, alive: reference.deref() !== undefined })),
    acquisitionScopes,
    acquisitionContexts,
    resourceAlive: resourceRefs.map((reference) => reference.deref() !== undefined),
    controlAlive: control.deref() !== undefined,
    remainingItems: pool.state.items.size
  }))
  if (finalized !== 2) throw new Error(`expected two finalizers; got ${finalized}`)
  if (pool.state.items.size !== 1) throw new Error(`expected min size 1; got ${pool.state.items.size}`)
  return yield* Effect.void
})))
