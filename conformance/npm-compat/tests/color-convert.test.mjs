// kiln eval smoke test (authored for kiln, not from porffor).
import convert from 'color-convert';
console.log(convert.rgb.hex(255, 0, 0));
console.log(JSON.stringify(convert.hex.rgb('FF0000')));
console.log(JSON.stringify(convert.rgb.hsl(255, 0, 0)));
console.log(JSON.stringify(convert.hsl.rgb(0, 100, 50)));
console.log(convert.keyword.rgb('red').join(','));
console.log(JSON.stringify(convert.rgb.keyword(255, 0, 0)));
