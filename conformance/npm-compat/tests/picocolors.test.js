// kiln eval smoke test (authored for kiln, not from porffor).
const pc = require('picocolors');

console.log(JSON.stringify(pc.red('hello')));
console.log(JSON.stringify(pc.bold(pc.green('ok'))));
console.log(JSON.stringify(pc.cyan(pc.underline('link'))));
console.log(pc.isColorSupported);
console.log(JSON.stringify(pc.bgBlue(pc.white('nesting works'))));
