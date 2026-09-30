// kiln eval smoke test (authored for kiln, not from porffor).
const { createStore, combineReducers, applyMiddleware } = require('redux');

function counter(state = 0, action) {
  switch (action.type) {
    case 'INC': return state + 1;
    case 'ADD': return state + action.n;
    default: return state;
  }
}
const store = createStore(counter);
const states = [];
store.subscribe(() => states.push(store.getState()));
store.dispatch({ type: 'INC' });
store.dispatch({ type: 'ADD', n: 5 });
store.dispatch({ type: 'UNKNOWN' });
console.log('states:', JSON.stringify(states));
console.log('final:', store.getState());

function todos(state = [], action) {
  return action.type === 'ADD_TODO' ? [...state, action.text] : state;
}
const root = combineReducers({ counter, todos });
const store2 = createStore(root);
store2.dispatch({ type: 'ADD_TODO', text: 'write tests' });
console.log('combined:', JSON.stringify(store2.getState()));

const logged = [];
const logger = () => (next) => (action) => { logged.push(action.type); return next(action); };
const store3 = createStore(counter, applyMiddleware(logger));
store3.dispatch({ type: 'INC' });
console.log('middleware saw:', JSON.stringify(logged), 'state:', store3.getState());

console.log('replaceReducer:', typeof store.replaceReducer === 'function');
