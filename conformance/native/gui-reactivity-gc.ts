// Bounded collector-stress companion to gui-reactivity.ts. The ordinary probe retains
// the full 100,000-round write-cycle guard; this probes values crossing reactive callbacks,
// owner disposal, keyed mapping and an asynchronous memo without that cost multiplier.
import {
  createEffect,
  createMemo,
  createRoot,
  createSignal,
  isPending,
  mapArray,
  onCleanup,
} from "kiln:gui";

const events: string[] = [];
createRoot((dispose) => {
  const [source, setSource] = createSignal({ value: 1 });
  const doubled = createMemo(() => ({ value: source().value * 2 }));
  createEffect(() => {
    const value = doubled().value;
    events.push(`value:${value}`);
    onCleanup(() => events.push(`cleanup:${value}`));
  });
  setSource({ value: 2 });
  setSource({ value: 3 });

  const [items, setItems] = createSignal(["a", "b"]);
  const mapped = mapArray(items, (item, index) => ({ item, index }));
  const before = mapped();
  setItems(["b", "a", "c"]);
  const after = mapped();
  console.log("identity:", after[0] === before[1], after[1] === before[0]);
  console.log("indexes:", after.map((entry) => `${entry.item}:${entry.index()}`).join(","));
  dispose();
  setSource({ value: 4 });
});
console.log("ownership:", events.join(","));

const [id, setId] = createSignal(1);
const result = createMemo(() => Promise.resolve({ value: id() }), { value: 0 });
const asynchronous: string[] = [];
createEffect(() => asynchronous.push(`${result().value}:${isPending(result)}`));
setId(2);
setTimeout(() => console.log("async:", asynchronous.join(",")), 0);
