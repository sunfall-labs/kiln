// kiln eval smoke test (authored for kiln, not from porffor).
const c = require('ansi-colors');

console.log('red:', JSON.stringify(c.red('error')));
console.log('bold:', JSON.stringify(c.bold('strong')));
console.log('chained:', JSON.stringify(c.red.bold('bad')));
console.log('bg:', JSON.stringify(c.bgBlue.white('box')));
console.log('stripColor:', c.stripColor(c.red('plain')));
console.log('unstyle:', c.unstyle(c.green('x')));
console.log('enabled flag:', typeof c.enabled);
console.log('noop:', JSON.stringify(c.noop('raw')), 'none:', JSON.stringify(c.none('x')));
