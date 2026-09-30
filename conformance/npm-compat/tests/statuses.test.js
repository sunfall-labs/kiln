// kiln eval smoke test (authored for kiln, not from porffor).
const statuses = require('statuses');

console.log(statuses(200));
console.log(statuses(404));
console.log(statuses('Not Found'));
console.log(statuses.codes[418]);
console.log(statuses.empty[204], statuses.redirect[301], statuses.retry[503]);
