// kiln eval smoke test (authored for kiln, not from porffor).
import Color from 'color';
console.log(Color('#ff0000').rgb().string());
console.log(Color('rebeccapurple').hsl().string());
console.log(Color('#00ff00').lighten(0.5).hex());
console.log(Color('#0000ff').negate().hex());
console.log(Color('#ffffff').luminosity().toFixed(3));
