const tinycolor = require('tinycolor2');

console.log('--- Multi-format parsing & Alpha ---');
console.log('hex8:', tinycolor('#ff000080').toHex8String());
console.log('rgba obj:', tinycolor({ r: 0, g: 255, b: 0, a: 0.5 }).toRgbString());
console.log('hsl string:', tinycolor('hsl(120, 100%, 50%)').toHexString());
console.log('hsv obj:', tinycolor({ h: 240, s: 1, v: 1 }).toName());

console.log('--- Color modifications ---');
const blue = tinycolor('#0000ff');
console.log('lighten 20:', blue.clone().lighten(20).toHexString());
console.log('darken 20:', blue.clone().darken(20).toHexString());
console.log('saturate 20:', tinycolor('#8080ff').saturate(20).toHexString());
console.log('desaturate 20:', blue.clone().desaturate(20).toHexString());
console.log('greyscale:', blue.clone().greyscale().toHexString());
console.log('complement:', tinycolor('red').complement().toHexString());
console.log('spin 180:', tinycolor('red').spin(180).toHexString());

console.log('--- Palette combinations ---');
const triad = tinycolor('red').triad().map(c => c.toHexString());
console.log('triad count:', triad.length, triad.join(', '));

const mono = tinycolor('blue').monochromatic(3).map(c => c.toHexString());
console.log('monochromatic count:', mono.length);

console.log('--- Readability & Contrast ---');
console.log('readability black/white:', tinycolor.readability('#000000', '#ffffff'));
console.log('isReadable AA:', tinycolor.isReadable('#000000', '#ffffff', { level: 'AA' }));
console.log('isReadable low contrast:', tinycolor.isReadable('#777777', '#888888'));
const most = tinycolor.mostReadable('#000', ['#111', '#555', '#fff']).toHexString();
console.log('mostReadable against black:', most);

console.log('--- Validation & invalid handling ---');
console.log('valid:', tinycolor('#123456').isValid());
console.log('invalid:', tinycolor('not-a-color-xyz').isValid());
console.log('invalid toHex:', tinycolor('not-a-color-xyz').toHexString());
