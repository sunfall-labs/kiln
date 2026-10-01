// kiln eval smoke test (authored for kiln, not from porffor).
const { configureStore, createSlice, createAsyncThunk } = require('@reduxjs/toolkit');

const counterSlice = createSlice({
  name: 'counter',
  initialState: { value: 0 },
  reducers: {
    increment: (state) => { state.value += 1; },
    addBy: (state, action) => { state.value += action.payload; }
  }
});
const store = configureStore({ reducer: { counter: counterSlice.reducer } });
store.dispatch(counterSlice.actions.increment());
store.dispatch(counterSlice.actions.addBy(5));
console.log('counter:', store.getState().counter.value);
console.log('action type:', counterSlice.actions.addBy(3).type);

const fetchThing = createAsyncThunk('thing/fetch', async (id) => ({ id, name: 'n' + id }));
const thingSlice = createSlice({
  name: 'thing',
  initialState: { status: 'idle', item: null },
  reducers: {},
  extraReducers: (builder) => {
    builder
      .addCase(fetchThing.pending, (s) => { s.status = 'loading'; })
      .addCase(fetchThing.fulfilled, (s, a) => { s.status = 'done'; s.item = a.payload; });
  }
});
const store2 = configureStore({ reducer: { thing: thingSlice.reducer } });
const p = store2.dispatch(fetchThing(7));
console.log('pending status:', store2.getState().thing.status);
p.then(() => {
  console.log('fulfilled:', JSON.stringify(store2.getState().thing));
  console.log('createSelector-ish select:', typeof store2.getState === 'function');
});
