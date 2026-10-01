// kiln eval smoke test (authored for kiln, not from porffor).
const {observable, autorun, computed, runInAction} = require('mobx');
const o = observable({n: 1});
const seen = [];
autorun(() => { seen.push(o.n * 2); });
runInAction(() => { o.n = 5; });
console.log(seen.join(','));
console.log(computed(() => o.n + 1).get());
const arr = observable([1, 2]);
runInAction(() => { arr.push(3); });
console.log(arr.length, arr.join(','));
