// kiln eval smoke test (authored for kiln, not from porffor).
const {red, green, bold, yellow, stripColors} = require('kolorist');
console.log(JSON.stringify(red('r')));
console.log(stripColors(red('r')));
console.log(JSON.stringify(bold(yellow('y'))));
console.log(green('g').length >= 1);
console.log(stripColors(bold('x')) === 'x');
