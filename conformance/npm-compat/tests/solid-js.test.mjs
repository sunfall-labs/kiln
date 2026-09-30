// kiln eval smoke test (authored for kiln, not from porffor).
import { createSignal, createEffect, createMemo, batch } from 'solid-js';

const [count, setCount] = createSignal(0);
console.log(count());
setCount(5);
console.log(count());

const doubled = createMemo(() => count() * 2);
console.log(doubled());
setCount(10);
console.log(doubled());

batch(() => {
  setCount(20);
  setCount(21);
});
console.log(count());

const log = [];
createEffect(() => log.push(count()));
setCount(42);
console.log(log.length >= 1, log[log.length - 1] === 42);
