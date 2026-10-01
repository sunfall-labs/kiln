// kiln eval smoke test (authored for kiln, not from porffor).
import chroma from 'chroma-js';
console.log(chroma('red').hex());
console.log(chroma('#00ff00').css());
console.log(chroma.mix('red', 'blue', 0.5, 'rgb').hex());
console.log(chroma('white').darken(2).hex());
console.log(chroma.scale(['red', 'blue'])(0.5).hex());
console.log(chroma.contrast('black', 'white').toFixed(2));
