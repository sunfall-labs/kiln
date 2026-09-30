// kiln eval smoke test (authored for kiln, not from porffor).
const { Effect } = require('effect');
console.log(Effect.runSync(Effect.succeed(41).pipe(Effect.map(x => x + 1))));
console.log(Effect.runSync(Effect.gen(function* () {
  const a = yield* Effect.succeed(2);
  const b = yield* Effect.succeed(21);
  return a * b;
})));
console.log(Effect.runSync(Effect.fail('boom').pipe(Effect.catchAll(e => Effect.succeed('caught:' + e)))));
console.log(Effect.runSync(Effect.all([Effect.succeed(1), Effect.succeed(2)]).pipe(Effect.map(([a, b]) => a + b))));
