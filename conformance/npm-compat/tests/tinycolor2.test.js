// kiln eval smoke test (authored for kiln, not from porffor).
const tinycolor = require('tinycolor2');
console.log(tinycolor('red').toHexString());
console.log(tinycolor('#00ff00').toRgbString());
console.log(tinycolor('red').lighten(10).toHexString());
console.log(tinycolor('rgb(0, 0, 255)').toHsvString());
console.log(tinycolor('#ffffff').isLight(), tinycolor('#000000').isDark());
console.log(tinycolor('notacolor').isValid());
