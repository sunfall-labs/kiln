// kiln eval smoke test (authored for kiln, not from porffor).
const parse = require('range-parser');
console.log(JSON.stringify(parse(1000, 'bytes=0-499')));
console.log(JSON.stringify(parse(1000, 'bytes=-500')));
console.log(JSON.stringify(parse(1000, 'bytes=500-')));
console.log(JSON.stringify(parse(1000, 'bytes=0-0,-1')));
console.log(JSON.stringify(parse(1000, 'items=0-10')));
console.log(parse(1000, 'bytes=2000-3000'));
