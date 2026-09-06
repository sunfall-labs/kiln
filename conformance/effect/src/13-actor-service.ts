// Actor service lifecycle, mailbox serialization and Scope cleanup
// expect: ACTOR 10 | 25 | closed
import { Effect, Context, Scope } from "effect";
import { spawnActorLayer } from "@kiln/effect";

class CounterActor extends Context.Service<CounterActor, {
  add: (n: number) => Effect.Effect<number>;
  get: () => Effect.Effect<number>;
}>()("CounterActor") {}

let closed = false;

const CounterLive = spawnActorLayer(CounterActor, () => {
  let value = 0;
  return {
    add: (n: number) => Effect.sync(() => {
      value += n;
      return value;
    }),
    get: () => Effect.sync(() => value),
    __shutdown: () => {
      closed = true;
    }
  };
});

const program = Effect.gen(function* () {
  const actor = yield* CounterActor;
  const v1 = yield* actor.add(10);
  const v2 = yield* actor.add(15);
  return `${v1} | ${v2}`;
});

const run = Effect.scoped(
  Effect.provide(program, CounterLive)
);

Effect.runPromise(run).then((res) => {
  console.log(`ACTOR ${res} | ${closed ? "closed" : "open"}`);
});
