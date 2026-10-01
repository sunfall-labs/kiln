// kiln eval smoke test (authored for kiln, not from porffor).
const invariant = require('invariant');
invariant(1 + 1 === 2, 'math broke');
console.log('truthy passed');
try { invariant(false, 'Amount must be positive, got %s', -5); } catch (e) { console.log(e.name + ': ' + e.message); }
try { invariant(0, 'zero is falsy'); } catch (e) { console.log(e.message); }
console.log(invariant(true, 'fine') === undefined);
