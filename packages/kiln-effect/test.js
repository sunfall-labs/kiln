import assert from "node:assert";
import { Effect, Context, Layer, Scope } from "effect";
import { spawnActorLayer, isolate, transfer, forEachParallel } from "./index.js";

// 1. Service Definition
class MeshService extends Context.Service()("MeshService", {
  accessors: true,
}) {}

// 2. Spawn Actor Layer
const MeshServiceLive = spawnActorLayer(MeshService, () => {
  let count = 0;
  return {
    process: (mesh) => Effect.sync(() => {
      count += mesh.length;
      return count;
    }),
    getCount: () => Effect.sync(() => count),
    __shutdown: () => {
      // Finalizer invoked on Scope closure
    }
  };
});

async function run() {
  // Test actor layer with Scope
  const program = Effect.gen(function* () {
    const service = yield* MeshService;
    const r1 = yield* service.process(new Uint8Array([1, 2, 3]));
    assert.strictEqual(r1, 3);
    const r2 = yield* service.process(new Uint8Array([4, 5]));
    assert.strictEqual(r2, 5);
    const total = yield* service.getCount();
    assert.strictEqual(total, 5);
    return total;
  });

  const scopedEffect = Effect.scoped(
    Effect.provide(program, MeshServiceLive)
  );

  const result = await Effect.runPromise(scopedEffect);
  assert.strictEqual(result, 5);

  // Test isolate and transfer
  const data = { vertices: 100 };
  const iso = await Effect.runPromise(isolate(data));
  const transferred = transfer(iso);
  assert.deepStrictEqual(transferred, { vertices: 100 });

  // Test forEachParallel
  const items = [10, 20, 30];
  const mapped = await Effect.runPromise(
    forEachParallel(items, (n) => Effect.succeed(n * 2), { concurrency: "native" })
  );
  assert.deepStrictEqual(mapped, [20, 40, 60]);

  console.log("All @kiln/effect tests passed!");
}

run();
