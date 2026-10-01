// kiln npm functional test: zustand
const { createStore } = require('zustand/vanilla');

// 1. Store with custom logging middleware
const log = [];
const logMiddleware = (config) => (set, get, api) =>
  config(
    (...args) => {
      const prev = get();
      set(...args);
      log.push({ prevCount: prev.cart ? prev.cart.items.length : 0, nextCount: get().cart.items.length });
    },
    get,
    api
  );

// 2. Multi-slice state composition
const createStoreWithSlices = (set, get) => ({
  user: { name: 'Guest', loggedIn: false },
  cart: { items: [], total: 0 },

  // Actions using get() and functional set()
  login: (name) => {
    set((state) => ({ user: { name, loggedIn: true } }));
  },
  logout: () => {
    set({ user: { name: 'Guest', loggedIn: false } });
  },
  addItem: (item) => {
    const current = get().cart;
    const newItems = [...current.items, item];
    const newTotal = newItems.reduce((sum, i) => sum + i.price, 0);
    set({ cart: { items: newItems, total: newTotal } });
  },
  // Full state replacement with second argument true
  replaceWholeState: (newState) => {
    set(newState, true);
  }
});

const store = createStore(logMiddleware(createStoreWithSlices));

// 3. Selective subscription tracking
const cartChanges = [];
let lastCartTotal = store.getState().cart.total;
const unsub = store.subscribe((state) => {
  if (state.cart && state.cart.total !== lastCartTotal) {
    lastCartTotal = state.cart.total;
    cartChanges.push(lastCartTotal);
  }
});

// Trigger state changes
store.getState().login('Ada');
console.log('user-after-login:', JSON.stringify(store.getState().user));

store.getState().addItem({ id: 101, name: 'Compiler', price: 50 });
store.getState().addItem({ id: 102, name: 'Profiler', price: 30 });
console.log('cart-after-items:', JSON.stringify(store.getState().cart));

// Unsubscribe and perform another update
unsub();
store.getState().addItem({ id: 103, name: 'Linter', price: 20 });
console.log('cart-after-unsub:', store.getState().cart.items.length, store.getState().cart.total);
console.log('selective-cart-notifications:', JSON.stringify(cartChanges));
console.log('middleware-recorded-transitions:', JSON.stringify(log));

// 4. Test replace state
store.getState().replaceWholeState({
  user: { name: 'Root', loggedIn: true },
  cart: { items: [], total: 0 }
});
console.log('after-replace-user:', store.getState().user.name);
console.log('after-replace-has-addItem:', typeof store.getState().addItem === 'undefined');
