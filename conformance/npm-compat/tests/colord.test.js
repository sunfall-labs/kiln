// kiln eval smoke test (authored for kiln, not from porffor).
const { colord } = require('colord');
console.log(colord('#ff0000').toHex());
console.log(JSON.stringify(colord('#ff0000').toRgb()));
console.log(colord('#000000').isDark(), colord('#ffffff').isLight());
console.log(colord('#ff0000').alpha(0.5).toRgbString());
console.log(colord('rebeccapurple').toHex());
console.log(colord('#00ff00').lighten(0.5).toHex());
