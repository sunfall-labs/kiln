// kiln eval smoke test (authored for kiln, not from porffor).
import { dinero, toSnapshot, add, multiply, isPositive, compare, equal } from 'dinero.js';
const d = dinero({ amount: 1050, currency: 'USD', scale: 2 });
console.log(JSON.stringify(toSnapshot(d)));
console.log(JSON.stringify(toSnapshot(add(d, dinero({ amount: 100, currency: 'USD', scale: 2 })))));
console.log(JSON.stringify(toSnapshot(multiply(d, 3))));
console.log(isPositive(d), compare(d, d), equal(d, d));
console.log(isPositive(dinero({ amount: -5, currency: 'USD', scale: 2 })));
