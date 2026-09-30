// kiln eval smoke test (authored for kiln, not from porffor).
import { red, bold, bgBlue, underline, green, yellow } from 'colorette';
console.log(JSON.stringify(red('x')));
console.log(JSON.stringify(bold(red('y'))));
console.log(JSON.stringify(bgBlue('z')));
console.log(JSON.stringify(underline(green('g'))));
console.log(JSON.stringify(yellow('plain')));
