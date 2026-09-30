// kiln eval smoke test (authored for kiln, not from porffor).
const { createSelector, createStructuredSelector } = require('reselect');

const state = { items: [{ price: 10, qty: 2 }, { price: 5, qty: 3 }], tax: 0.1 };
const selectItems = (s) => s.items;
const selectTax = (s) => s.tax;

let computes = 0;
const selectTotal = createSelector([selectItems], (items) => {
  computes++;
  return items.reduce((sum, i) => sum + i.price * i.qty, 0);
});
console.log('total:', selectTotal(state), 'computes:', computes);
console.log('memoized:', selectTotal(state), 'computes still:', computes);

const selectWithTax = createSelector([selectTotal, selectTax], (total, tax) => total * (1 + tax));
console.log('with tax:', selectWithTax(state));

const structured = createStructuredSelector({ total: selectTotal, tax: selectTax });
console.log('structured:', JSON.stringify(structured(state)));

const selectDouble = createSelector([(s) => s.n], (n) => n * 2);
console.log('recomputations:', selectDouble.recomputations());
selectDouble({ n: 21 });
console.log('after call:', selectDouble({ n: 21 }), 'recomputations:', selectDouble.recomputations());
