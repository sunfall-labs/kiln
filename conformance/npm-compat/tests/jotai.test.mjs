// kiln eval smoke test (authored for kiln, not from porffor).
import { atom, createStore } from 'jotai/vanilla';

const store = createStore();
const countAtom = atom(0);
store.set(countAtom, 5);
console.log(store.get(countAtom));
const doubledAtom = atom((get) => get(countAtom) * 2);
console.log(store.get(doubledAtom));
store.set(countAtom, 10);
console.log(store.get(countAtom), store.get(doubledAtom));
const seen = [];
const unsub = store.sub(countAtom, () => seen.push(store.get(countAtom)));
store.set(countAtom, 11);
unsub();
store.set(countAtom, 12);
console.log(seen.join(','), 'final', store.get(countAtom));
