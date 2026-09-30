// kiln eval smoke test (authored for kiln, not from porffor).
import { rgb, formatHex, parseHex, formatCss } from 'culori';
console.log(JSON.stringify(rgb('red')));
console.log(formatHex('rebeccapurple'));
console.log(JSON.stringify(parseHex('#ff8800')));
console.log(formatCss(rgb('#00ff00')));
console.log(JSON.stringify(rgb('rgb(10, 20, 30)')));
