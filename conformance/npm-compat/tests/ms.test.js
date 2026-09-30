// kiln eval smoke test (authored for kiln, not from porffor).
const ms = require('ms');
console.log(ms('2 days'));
console.log(ms(60000));
console.log(ms('1h'));
console.log(ms(86400000));
console.log(ms('1.5h'));
console.log(ms('10 seconds'));
console.log(ms('not a time'));
console.log(ms(0));
