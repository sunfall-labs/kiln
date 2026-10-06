import { Effect, Fiber } from "effect"
import { TestClock } from "effect/testing"

let zeroDelayIterations = 0
let foregroundSleeps = 0

const refreshLike = Effect.gen(function*() {
  while (true) {
    zeroDelayIterations++
    // TestClock.sleep(0) returns immediately. This mirrors the refresh loop's
    // interval-zero branch and relies on Effect's operation-budget yield.
    yield* Effect.sleep(0)
  }
})

const foregroundPoll = Effect.forever(
  Effect.sleep(100).pipe(Effect.andThen(Effect.sync(() => foregroundSleeps++)))
)

const main = Effect.gen(function*() {
  const background = yield* refreshLike.pipe(Effect.forkChild({ startImmediately: true }))
  const poll = yield* foregroundPoll.pipe(Effect.forkChild({ startImmediately: true }))

  yield* TestClock.adjust(60_000)
  const finalTime = yield* TestClock.testClockWith((clock) =>
    Effect.sync(() => clock.currentTimeMillisUnsafe())
  )

  yield* Fiber.interrupt(poll)
  yield* Fiber.interrupt(background)

  console.log(JSON.stringify({ finalTime, foregroundSleeps, zeroDelayIterations }))
  if (finalTime !== 60_000 || foregroundSleeps !== 600 || zeroDelayIterations === 0) {
    throw new Error("zero-delay background work prevented expected TestClock progress")
  }
})

Effect.runPromise(Effect.provide(main, TestClock.layer())).catch((error) => {
  console.error(error)
  process.exitCode = 1
})
