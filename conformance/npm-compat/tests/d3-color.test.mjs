// kiln eval smoke test (authored for kiln, not from porffor).
import { rgb, hsl } from 'd3-color';
console.log(rgb('#ff0000').formatHex());
console.log(hsl('rebeccapurple').formatHsl());
console.log(rgb('red').brighter(1).formatHex());
console.log(rgb('rebeccapurple').formatRgb());
console.log(rgb('#000').darker(2).formatHex());
