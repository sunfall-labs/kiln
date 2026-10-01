// kiln eval smoke test (authored for kiln, not from porffor).
const minimist = require('minimist');
console.log(JSON.stringify(minimist(['--name', 'ada', '--count=3', 'pos1', '-v'])));
console.log(JSON.stringify(minimist(['-v', '--no-cache'], { boolean: ['v', 'cache'], default: { count: 1 } })));
console.log(JSON.stringify(minimist(['--list', 'a', '--list', 'b']).list));
console.log(JSON.stringify(minimist(['-abc'])._));
