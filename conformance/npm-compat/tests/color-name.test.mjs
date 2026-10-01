// kiln eval smoke test (authored for kiln, not from porffor).
import colors from 'color-name';
console.log(JSON.stringify(colors.red));
console.log(JSON.stringify(colors.rebeccapurple));
console.log(JSON.stringify(colors.goldenrod));
console.log(Object.keys(colors).length);
console.log(Object.isFrozen(colors.red));
