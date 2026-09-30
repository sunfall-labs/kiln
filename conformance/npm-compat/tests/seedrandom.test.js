// kiln eval smoke test (authored for kiln, not from porffor).
const seedrandom = require('seedrandom');
const rng = seedrandom('hello');
console.log(rng().toFixed(6));
console.log(rng().toFixed(6));
const rng2 = seedrandom('hello');
console.log(rng2().toFixed(6));
console.log(seedrandom('other')().toFixed(6));
const int = seedrandom('intseed');
console.log(Math.floor(int() * 100) >= 0);
