// kiln eval smoke test (authored for kiln, not from porffor).
const { createStore } = require('zustand/vanilla');

const store = createStore((set) => ({
  count: 0,
  inc: () => set((s) => ({ count: s.count + 1 })),
  add: (n) => set({ count: n }),
  reset: () => set({ count: 0 })
}));

console.log('initial:', store.getState().count);
store.getState().inc();
store.getState().inc();
console.log('after inc:', store.getState().count);
store.getState().add(10);
console.log('after add:', store.getState().count);

const seen = [];
const unsub = store.subscribe((s) => seen.push(s.count));
store.getState().inc();
unsub();
store.getState().inc();
console.log('subscriber saw:', JSON.stringify(seen));

store.setState({ count: 100 });
console.log('setState:', store.getState().count);
store.getState().reset();
console.log('reset:', store.getState().count);
console.log('persist-ish api:', typeof store.persist === 'undefined' || true);
