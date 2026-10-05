import { describe, it } from "@effect/vitest";
import * as Effect from "effect/Effect";
import * as Schedule from "effect/Schedule";
import * as Scheduler from "effect/Scheduler";
import * as Stream from "effect/Stream";
import * as Channel from "effect/Channel";

const countOps = <A, E>(effect: Effect.Effect<A, E>): Effect.Effect<number, E> =>
  Effect.suspend(() => {
    let ops = 0;
    const scheduler: Scheduler.Scheduler = {
      executionMode: "sync",
      makeDispatcher: () => new Scheduler.MixedScheduler("sync").makeDispatcher(),
      shouldYield: () => { ops++; return false; }
    };
    return Effect.map(Effect.provideService(effect, Scheduler.Scheduler, scheduler), () => ops);
  });

const run = (kind: "repeat" | "forever", n: number) => {
  const source = Channel.succeed([1]);
  const channel = kind === "repeat" ? Channel.repeat(source, Schedule.forever) : Channel.forever(source);
  const stream = Stream.fromChannel(channel);
  return countOps(stream.pipe(Stream.take(n), Stream.runDrain));
};

describe("Channel repetition diagnostic", () => {
  it.effect("collect scaled operation counts", () =>
    Effect.gen(function*() {
      const rows: Array<{kind:string,n:number,ops:number}> = [];
      for (const kind of ["forever", "repeat"] as const) {
        for (const n of [125, 250, 500]) {
          const ops = yield* run(kind, n);
          rows.push({kind, n, ops});
        }
      }
      throw new Error("DIAGNOSTIC_OP_COUNTS " + JSON.stringify(rows));
    }), 30000);
});
