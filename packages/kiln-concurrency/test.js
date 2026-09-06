import assert from "node:assert";
import { Actor, spawnActor, isolate, transfer } from "./index.js";

class Counter extends Actor {
  #count = 0;
  
  increment(amount = 1) {
    this.#count += amount;
    return this.#count;
  }
  
  getCount() {
    return this.#count;
  }
}

async function run() {
  const counter = spawnActor(Counter);
  assert.strictEqual(typeof counter.increment, "function");
  
  const c1 = await counter.increment(5);
  assert.strictEqual(c1, 5);
  
  const c2 = await counter.increment(3);
  assert.strictEqual(c2, 8);
  
  const total = await counter.getCount();
  assert.strictEqual(total, 8);

  // Region isolation & transfer
  const data = { x: 10, y: 20 };
  const iso = isolate(data);
  const transferred = transfer(iso);
  assert.deepStrictEqual(transferred, { x: 10, y: 20 });

  // parallelMap
  const arr = [1, 2, 3, 4];
  const squares = await arr.parallelMap(x => x * x);
  assert.deepStrictEqual(squares, [1, 4, 9, 16]);

  console.log("All @kiln/concurrency tests passed!");
}

run();
