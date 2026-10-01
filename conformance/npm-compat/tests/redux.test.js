// kiln npm functional test: redux
const { createStore, combineReducers, applyMiddleware } = require('redux');

// 1. Reducers with immutable state updates & reference preservation
const initialUsers = [{ id: 1, name: 'Ada' }, { id: 2, name: 'Babbage' }];
function usersReducer(state = initialUsers, action) {
  switch (action.type) {
    case 'USER_ADD':
      return [...state, action.payload];
    case 'USER_RENAME':
      return state.map(u => u.id === action.payload.id ? { ...u, name: action.payload.name } : u);
    default:
      return state;
  }
}

function filterReducer(state = 'ALL', action) {
  switch (action.type) {
    case 'FILTER_SET':
      return action.payload;
    default:
      return state;
  }
}

const rootReducer = combineReducers({
  users: usersReducer,
  filter: filterReducer
});

// 2. Chained middleware (logging + thunk dispatch)
const log = [];
const actionLogger = store => next => action => {
  log.push('before:' + (typeof action === 'function' ? 'thunk' : action.type));
  const res = next(action);
  log.push('after:' + (typeof action === 'function' ? 'thunk' : action.type));
  return res;
};

const thunkMiddleware = ({ dispatch, getState }) => next => action => {
  if (typeof action === 'function') {
    return action(dispatch, getState);
  }
  return next(action);
};

const store = createStore(rootReducer, applyMiddleware(thunkMiddleware, actionLogger));

// 3. Multi-subscriber lifecycle & unsubscription during notification
const history = [];
let notifyCount = 0;
const unsub1 = store.subscribe(() => {
  notifyCount++;
  history.push({ step: notifyCount, users: store.getState().users.length });
  if (notifyCount === 2) {
    unsub1(); // unsubscribes itself on second call
  }
});

const unsub2 = store.subscribe(() => {
  // secondary subscriber
});

// Dispatch direct action
const s0 = store.getState();
store.dispatch({ type: 'USER_ADD', payload: { id: 3, name: 'Turing' } });
const s1 = store.getState();
console.log('ref-equality-filter:', s0.filter === s1.filter);
console.log('ref-inequality-users:', s0.users === s1.users);

// Dispatch thunk action
const asyncAdd = (user) => (dispatch, getState) => {
  const currentCount = getState().users.length;
  if (currentCount < 5) {
    dispatch({ type: 'USER_ADD', payload: user });
  }
};
store.dispatch(asyncAdd({ id: 4, name: 'Lovelace' }));
store.dispatch(asyncAdd({ id: 5, name: 'Shannon' }));

console.log('notify-history:', JSON.stringify(history));
console.log('final-user-count:', store.getState().users.length);
console.log('middleware-log:', JSON.stringify(log));

// 4. replaceReducer runtime migration
const newReducer = combineReducers({
  users: usersReducer,
  filter: filterReducer,
  extra: (state = 'loaded', action) => state
});
store.replaceReducer(newReducer);
console.log('replaced-state:', JSON.stringify(store.getState()));
